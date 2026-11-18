//! chaat-relay: forwards Microsoft Graph change notifications to Chaat.
//!
//! Graph can only push change notifications to a public HTTPS endpoint, and a
//! desktop app has none. This relay is that endpoint, small enough for
//! anyone to run behind their own TLS proxy:
//!
//! - Chaat opens a WebSocket at `/ws/{channel}`. The channel is a random token
//!   Chaat makes for each run.
//! - Chaat creates a Graph subscription whose `notificationUrl` is
//!   `/graph/{channel}` and whose `lifecycleNotificationUrl` is
//!   `/lifecycle/{channel}`.
//! - Graph validates each URL with a `validationToken`, which the relay echoes
//!   back. Graph then posts notifications there, and the relay hands each body
//!   to the channel's WebSocket and answers `202 Accepted` at once.
//!
//! Chaat subscribes without resource data, so notifications carry only IDs
//! (chat and message), never message content. Chaat fetches the content
//! itself from Graph. The relay keeps nothing on disk and forgets a channel
//! when its WebSocket closes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use tokio::sync::mpsc;

/// Graph notification batches are small; anything bigger is not Graph.
const BODY_LIMIT: usize = 512 * 1024;
/// How many notifications may wait for a slow client before new ones drop.
const QUEUE: usize = 256;
/// Clients only send pings, so their messages are tiny.
const CLIENT_MESSAGE_LIMIT: usize = 4 * 1024;

/// How many clients a relay serves at once, and how long a silent one keeps
/// its channel.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Connections beyond this are refused with `503`.
    pub max_clients: usize,
    /// Chaat pings every 30 seconds; a client silent this long is gone
    /// (a sleeping laptop, a dropped network) and its channel is freed.
    pub idle: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_clients: 1000,
            idle: Duration::from_secs(90),
        }
    }
}

#[derive(Clone, Default)]
pub struct Relay {
    channels: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    limits: Limits,
}

impl Relay {
    pub fn new(limits: Limits) -> Self {
        Self {
            channels: Arc::default(),
            limits,
        }
    }

    pub fn connected(&self) -> usize {
        self.channels
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }
}

/// The relay's routes.
pub fn router(relay: Relay) -> Router {
    Router::new()
        .route("/", get(|| async { "chaat-relay ok\n" }))
        .route("/ws/{channel}", get(connect))
        .route("/graph/{channel}", post(notification))
        .route("/lifecycle/{channel}", post(notification))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(relay)
}

/// Channels are Chaat's random tokens: long, URL-safe, nothing else.
fn valid_channel(channel: &str) -> bool {
    (32..=128).contains(&channel.len())
        && channel
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn connect(
    ws: WebSocketUpgrade,
    Path(channel): Path<String>,
    State(relay): State<Relay>,
) -> Response {
    if !valid_channel(&channel) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let full = {
        let channels = relay.channels.lock().unwrap_or_else(|p| p.into_inner());
        // A reconnect replaces its own channel, so it never counts as new.
        channels.len() >= relay.limits.max_clients && !channels.contains_key(&channel)
    };
    if full {
        log::warn!("refused a client: {} connected", relay.limits.max_clients);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    ws.max_message_size(CLIENT_MESSAGE_LIMIT)
        .max_frame_size(CLIENT_MESSAGE_LIMIT)
        .on_upgrade(move |socket| serve_client(socket, channel, relay))
}

async fn serve_client(mut socket: WebSocket, channel: String, relay: Relay) {
    let (sender, mut receiver) = mpsc::channel::<String>(QUEUE);
    relay
        .channels
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(channel.clone(), sender.clone());
    log::info!("client connected ({} connected)", relay.connected());
    let idle = tokio::time::sleep(relay.limits.idle);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            () = &mut idle => {
                log::info!("client went silent; closing");
                break;
            }
            forwarded = receiver.recv() => {
                let Some(body) = forwarded else { break };
                if socket.send(Message::Text(body.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                // Clients only ping; anything else just shows they are there.
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => idle
                    .as_mut()
                    .reset(tokio::time::Instant::now() + relay.limits.idle),
            },
        }
    }
    let mut channels = relay.channels.lock().unwrap_or_else(|p| p.into_inner());
    // A reconnect may already have replaced this client; keep the new one.
    if channels
        .get(&channel)
        .is_some_and(|current| current.same_channel(&sender))
    {
        channels.remove(&channel);
    }
    log::info!("client disconnected ({} connected)", channels.len());
}

#[derive(Deserialize)]
struct Validation {
    #[serde(rename = "validationToken")]
    validation_token: Option<String>,
}

async fn notification(
    Path(channel): Path<String>,
    Query(query): Query<Validation>,
    State(relay): State<Relay>,
    body: String,
) -> Response {
    if !valid_channel(&channel) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    // Graph checks a new notification URL by posting a token that must come
    // back as plain text within ten seconds.
    if let Some(token) = query.validation_token {
        return ([(header::CONTENT_TYPE, "text/plain")], token).into_response();
    }
    let sender = relay
        .channels
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&channel)
        .cloned();
    match sender {
        Some(sender) => {
            if sender.try_send(body).is_err() {
                log::warn!("a client is not keeping up; dropped a notification");
            }
        }
        None => log::info!("notification for a channel with no client; dropped"),
    }
    // Graph must hear back quickly whatever happens, or it slows down and
    // eventually drops the subscription.
    StatusCode::ACCEPTED.into_response()
}

/// Serves `app` on `listener` until the process ends.
pub async fn serve(listener: tokio::net::TcpListener, app: Router) {
    if let Err(error) = axum::serve(listener, app).await {
        log::error!("relay stopped: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    const CHANNEL: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    async fn start() -> (String, Relay) {
        start_with(Limits::default()).await
    }

    async fn start_with(limits: Limits) -> (String, Relay) {
        let relay = Relay::new(limits);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = router(relay.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("127.0.0.1:{}", address.port()), relay)
    }

    #[tokio::test]
    async fn validation_tokens_are_echoed_as_text() {
        let (host, _) = start().await;
        let response = reqwest::Client::new()
            .post(format!("http://{host}/graph/{CHANNEL}?validationToken=Validation%3A%20Testing%20client%20application"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "text/plain");
        assert_eq!(
            response.text().await.unwrap(),
            "Validation: Testing client application"
        );
    }

    #[tokio::test]
    async fn notifications_reach_the_channel_websocket() {
        let (host, relay) = start().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{host}/ws/{CHANNEL}"))
            .await
            .unwrap();
        // Wait until the relay has registered the channel.
        while relay.connected() == 0 {
            tokio::task::yield_now().await;
        }
        let body = r#"{"value":[{"clientState":"s","resource":"chats('19:a')/messages('1')"}]}"#;
        let status = reqwest::Client::new()
            .post(format!("http://{host}/graph/{CHANNEL}"))
            .body(body)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 202);
        let message = socket.next().await.unwrap().unwrap();
        assert_eq!(message.into_text().unwrap().as_str(), body);
    }

    #[tokio::test]
    async fn bad_channels_and_unknown_channels() {
        let (host, _) = start().await;
        let client = reqwest::Client::new();
        let short = client
            .post(format!("http://{host}/graph/short"))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(short.status(), 400);
        // Nobody listening: still accepted, so Graph keeps the subscription.
        let unknown = client
            .post(format!("http://{host}/lifecycle/{CHANNEL}"))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), 202);
    }

    async fn wait_for(relay: &Relay, connected: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while relay.connected() != connected {
            assert!(
                tokio::time::Instant::now() < deadline,
                "never reached {connected} connected"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn clients_beyond_the_limit_are_refused_but_reconnects_are_not() {
        let (host, relay) = start_with(Limits {
            max_clients: 1,
            ..Limits::default()
        })
        .await;
        let (_first, _) = tokio_tungstenite::connect_async(format!("ws://{host}/ws/{CHANNEL}"))
            .await
            .unwrap();
        wait_for(&relay, 1).await;
        let other = "b".repeat(40);
        match tokio_tungstenite::connect_async(format!("ws://{host}/ws/{other}")).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), 503)
            }
            other => panic!("expected 503, got {:?}", other.map(|_| ())),
        }
        // The same channel reconnecting (Chaat after a network change) still works.
        let (_again, _) = tokio_tungstenite::connect_async(format!("ws://{host}/ws/{CHANNEL}"))
            .await
            .unwrap();
        assert_eq!(relay.connected(), 1);
    }

    #[tokio::test]
    async fn silent_clients_are_closed_and_pinging_ones_stay() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as Frame;
        let (host, relay) = start_with(Limits {
            idle: Duration::from_millis(300),
            ..Limits::default()
        })
        .await;
        let (mut pinging, _) =
            tokio_tungstenite::connect_async(format!("ws://{host}/ws/{CHANNEL}"))
                .await
                .unwrap();
        let silent_channel = "c".repeat(40);
        let (mut silent, _) =
            tokio_tungstenite::connect_async(format!("ws://{host}/ws/{silent_channel}"))
                .await
                .unwrap();
        wait_for(&relay, 2).await;
        for _ in 0..8 {
            pinging.send(Frame::Ping(Vec::new().into())).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        wait_for(&relay, 1).await;
        // The silent socket ends; the pinging one is still registered.
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match silent.next().await {
                    Some(Ok(Frame::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        assert!(ended.is_ok());
        assert!(relay.channels.lock().unwrap().contains_key(CHANNEL));
    }

    #[tokio::test]
    async fn oversized_client_messages_close_the_socket() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as Frame;
        let (host, relay) = start().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{host}/ws/{CHANNEL}"))
            .await
            .unwrap();
        wait_for(&relay, 1).await;
        let _ = socket.send(Frame::Text("x".repeat(64 * 1024).into())).await;
        wait_for(&relay, 0).await;
    }
}
