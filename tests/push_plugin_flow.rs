//! Push through a plugin: Graph posts reach the backend as forwarded bodies
//! (`Command::PushBody`) instead of through a relay WebSocket. Graph and Entra
//! are wiremock stand-ins.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chaat::auth::session::AccountRecord;
use chaat::backend::{AuthStatus, Backend, BackendOptions, Command, Event, PushRoute, Waker};
use chaat::model::ConversationId;
use chaat::paths::AppDirs;
use chaat::realtime::PushStatus;
use chaat::secrets::{MemoryStore, SecretStore};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT: &str = "6731de76-14a6-49ae-97bc-6eba6914391e";
const ACCOUNT: &str = "u-me.t-1";
const CHAT: &str = "19:ada-me@unq.gbl.spaces";
const URL: &str = "https://a.trycloudflare.com";

struct Fixture {
    server: MockServer,
    backend: Backend,
    pending: Vec<Event>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn wait_for<T>(&mut self, seconds: u64, mut pick: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            self.pending.extend(self.backend.poll());
            while !self.pending.is_empty() {
                if let Some(found) = pick(self.pending.remove(0)) {
                    return found;
                }
            }
            assert!(Instant::now() < deadline, "timed out waiting for an event");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn live(&mut self) {
        self.wait_for(10, |e| match e {
            Event::Push(PushStatus::Live) => Some(()),
            Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
            _ => None,
        });
    }

    fn off(&mut self) {
        self.wait_for(5, |e| match e {
            Event::Push(PushStatus::Off) => Some(()),
            Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
            _ => None,
        });
    }

    async fn posts(&self) -> Vec<serde_json::Value> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/v1.0/subscriptions")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    async fn count(&self, verb: &str, route: &str) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == verb && r.url.path() == route)
            .count()
    }
}

fn route(url: &str) -> PushRoute {
    PushRoute::new(url.into(), "c".repeat(43))
}

async fn fixture() -> Fixture {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access", "refresh_token": "refresh-2", "expires_in": 3600,
            "scope": "User.Read Chat.ReadWrite ChatMessage.Send"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1.0/subscriptions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": "sub-1", "expirationDateTime": "2026-10-11T13:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1.0/subscriptions/sub-1"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/presence"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "u-me", "availability": "Available", "activity": "Available"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v1.0/chats/{CHAT}/messages")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                {"id": "1702", "messageType": "message", "createdDateTime": "2026-10-11T10:05:00Z",
                 "lastModifiedDateTime": "2026-10-11T10:05:00Z",
                 "from": {"user": {"id": "u-ada", "displayName": "Ada Lovelace"}},
                 "body": {"contentType": "text", "content": "Pushed!"}}
            ]})),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let dirs = AppDirs::under(dir.path());
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    secrets
        .write(&chaat::secrets::refresh_key(ACCOUNT), "refresh-1")
        .unwrap();
    std::fs::create_dir_all(dirs.account(ACCOUNT)).unwrap();
    AccountRecord {
        id: ACCOUNT.into(),
        display_name: "Alex Rivera".into(),
        username: "alex@contoso.com".into(),
        tenant_id: "t-1".into(),
        granted: "User.Read Chat.ReadWrite ChatMessage.Send".into(),
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
    }
    .save(&dirs.account(ACCOUNT).join("account.json"))
    .unwrap();
    let backend = Backend::spawn_with(
        dirs,
        Waker::default(),
        BackendOptions {
            authority: server.uri(),
            graph_base: format!("{}/v1.0", server.uri()),
            secrets,
            open_browser: false,
            push_renew_every: chaat::realtime::RENEW_EVERY,
            relay_connect_timeout: Duration::from_secs(20),
        },
    );
    backend.send(Command::Start {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        account: Some(ACCOUNT.into()),
    });
    let mut f = Fixture {
        server,
        backend,
        pending: Vec::new(),
        _dir: dir,
    };
    f.wait_for(5, |e| {
        matches!(e, Event::Auth(AuthStatus::SignedIn { .. })).then_some(())
    });
    f
}

fn body(client_state: &str) -> String {
    serde_json::json!({"value": [{
        "clientState": client_state,
        "resource": format!("chats('{CHAT}')/messages('1')"),
    }]})
    .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarded_body_fetches_the_chat() {
    let mut f = fixture().await;
    let route = route(URL);
    f.backend.send(Command::PushPlugin(Some(route.clone())));
    f.live();
    let posts = f.posts().await;
    assert_eq!(
        posts[0]["notificationUrl"],
        format!("{URL}/graph/{}", route.channel)
    );
    assert_eq!(
        posts[0]["lifecycleNotificationUrl"],
        format!("{URL}/lifecycle/{}", route.channel)
    );
    let state = posts[0]["clientState"].as_str().unwrap().to_owned();
    f.backend.send(Command::PushBody(body(&state)));
    let messages = f.wait_for(10, |e| match e {
        Event::Messages {
            conversation,
            messages,
            cached: false,
            ..
        } if conversation == ConversationId::Chat(CHAT.into()) => Some(messages),
        _ => None,
    });
    assert_eq!(messages[0].body.to_plain(), "Pushed!");
}

#[tokio::test(flavor = "multi_thread")]
async fn body_with_wrong_client_state_is_ignored() {
    let mut f = fixture().await;
    f.backend.send(Command::PushPlugin(Some(route(URL))));
    f.live();
    let chat = format!("/v1.0/chats/{CHAT}/messages");
    f.backend.send(Command::PushBody(body("not-the-state")));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(f.count("GET", &chat).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn body_without_a_plugin_loop_is_dropped() {
    let mut f = fixture().await;
    f.backend.send(Command::PushBody(body("x")));
    f.backend.send(Command::PushPlugin(None));
    f.off();
    assert_eq!(f.count("POST", "/v1.0/subscriptions").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn route_none_deletes_subscriptions() {
    let mut f = fixture().await;
    f.backend.send(Command::PushPlugin(Some(route(URL))));
    f.live();
    f.backend.send(Command::PushPlugin(None));
    f.off();
    assert!(f.count("DELETE", "/v1.0/subscriptions/sub-1").await >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn new_route_resubscribes() {
    let mut f = fixture().await;
    f.backend.send(Command::PushPlugin(Some(route(URL))));
    f.live();
    let other = "https://b.trycloudflare.com";
    f.backend.send(Command::PushPlugin(Some(route(other))));
    f.live();
    assert!(f.count("DELETE", "/v1.0/subscriptions/sub-1").await >= 1);
    let posts = f.posts().await;
    let last = posts.last().unwrap()["notificationUrl"].as_str().unwrap();
    assert!(last.starts_with(&format!("{other}/graph/")));
}

#[tokio::test(flavor = "multi_thread")]
async fn relay_wins_over_plugin_route() {
    let mut f = fixture().await;
    // A bare WebSocket acceptor stands in for the relay.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await {
                    use futures_util::StreamExt;
                    while ws.next().await.is_some() {}
                }
            });
        }
    });
    f.backend.send(Command::Relay(Some(relay.clone())));
    f.backend.send(Command::PushPlugin(Some(route(URL))));
    f.live();
    let posts = f.posts().await;
    assert!(
        posts
            .iter()
            .all(|p| p["notificationUrl"].as_str().unwrap().starts_with(&relay)),
        "every subscription uses the relay"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn route_change_with_relay_set_does_not_resubscribe() {
    let mut f = fixture().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await {
                    use futures_util::StreamExt;
                    while ws.next().await.is_some() {}
                }
            });
        }
    });
    f.backend.send(Command::Relay(Some(relay)));
    f.live();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = f.count("POST", "/v1.0/subscriptions").await;
    assert!(before >= 1);
    // What the interface sends when a relay is set by hand, and then a
    // plugin route coming and going: the relay session is not restarted.
    f.backend.send(Command::PushPlugin(None));
    f.backend.send(Command::PushPlugin(Some(route(URL))));
    f.backend.send(Command::PushPlugin(None));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(f.count("POST", "/v1.0/subscriptions").await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_the_route_is_not_reported_as_a_failure() {
    let mut f = fixture().await;
    for _ in 0..4 {
        f.backend.send(Command::PushPlugin(Some(route(URL))));
        f.live();
        f.backend.send(Command::PushPlugin(None));
        f.off();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    f.pending.extend(f.backend.poll());
    assert!(
        !f.pending
            .iter()
            .any(|e| matches!(e, Event::Push(PushStatus::Failed(_)))),
        "a stop must not surface as Failed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_of_bodies_is_bounded() {
    let mut f = fixture().await;
    // The first subscribe is slow, so the loop is not reading bodies while the
    // flood arrives. Only that one request is slow; the fixture's own fast
    // mock answers every later one.
    Mock::given(method("POST"))
        .and(path("/v1.0/subscriptions"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_delay(Duration::from_millis(1500))
                .set_body_json(serde_json::json!({
                    "id": "sub-1", "expirationDateTime": "2026-10-11T13:00:00Z"
                })),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&f.server)
        .await;

    f.backend.send(Command::PushPlugin(Some(route(URL))));
    for _ in 0..1000 {
        f.backend.send(Command::PushBody(body("junk")));
    }
    // The backend still answers a later command: stopping the route reports
    // Off. A full queue must not hold the command loop up. The deadline is
    // generous, since stopping waits for the slow subscribe (3 s at most).
    f.backend.send(Command::PushPlugin(None));
    f.wait_for(30, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });
    f.backend.send(Command::PushPlugin(Some(route(URL))));
    f.live();
    // A valid body after Live still fetches. The loop that was stopped while
    // subscribing may have posted too, so try every client state seen: only
    // the running loop's is accepted, and the others are ignored.
    let deadline = Instant::now() + Duration::from_secs(30);
    let messages = loop {
        let mut states: Vec<String> = f
            .posts()
            .await
            .iter()
            .filter_map(|post| post["clientState"].as_str().map(str::to_owned))
            .collect();
        states.sort();
        states.dedup();
        for state in &states {
            f.backend.send(Command::PushBody(body(state)));
        }
        let until = Instant::now() + Duration::from_millis(500);
        let mut found = None;
        while found.is_none() && Instant::now() < until {
            f.pending.extend(f.backend.poll());
            while !f.pending.is_empty() {
                if let Event::Messages {
                    conversation,
                    messages,
                    cached: false,
                    ..
                } = f.pending.remove(0)
                    && conversation == ConversationId::Chat(CHAT.into())
                {
                    found = Some(messages);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(messages) = found {
            break messages;
        }
        assert!(Instant::now() < deadline, "no fetch after a valid body");
    };
    assert_eq!(messages[0].body.to_plain(), "Pushed!");
}

fn lifecycle(client_state: &str, event: &str, subscription: &str) -> String {
    serde_json::json!({"value": [{
        "subscriptionId": subscription,
        "clientState": client_state,
        "lifecycleEvent": event,
    }]})
    .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn lifecycle_notices_count_only_for_held_subscriptions_and_are_limited() {
    let mut f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/v1.0/subscriptions/sub-1/reauthorize"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&f.server)
        .await;
    f.backend.send(Command::PushPlugin(Some(route(URL))));
    f.live();
    let state = f.posts().await[0]["clientState"]
        .as_str()
        .unwrap()
        .to_owned();
    let subscribed = f.count("POST", "/v1.0/subscriptions").await;
    // Someone else's subscription: neither reauthorized nor remade.
    for event in ["reauthorizationRequired", "subscriptionRemoved"] {
        f.backend
            .send(Command::PushBody(lifecycle(&state, event, "sub-other")));
    }
    // Ours, again and again: reauthorized once.
    for _ in 0..5 {
        f.backend.send(Command::PushBody(lifecycle(
            &state,
            "reauthorizationRequired",
            "sub-1",
        )));
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        f.count("POST", "/v1.0/subscriptions/sub-1/reauthorize")
            .await,
        1
    );
    assert_eq!(f.count("POST", "/v1.0/subscriptions").await, subscribed);
    assert_eq!(f.count("DELETE", "/v1.0/subscriptions/sub-1").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_fetches_from_pushes_stay_within_the_budget() {
    let mut f = fixture().await;
    f.backend.send(Command::PushPlugin(Some(route(URL))));
    f.live();
    let state = f.posts().await[0]["clientState"]
        .as_str()
        .unwrap()
        .to_owned();
    // Three bodies of 20 made-up chats each: 60 chats, a budget of 20 (plus
    // the little it refills meanwhile).
    for round in 0..3 {
        let items: Vec<serde_json::Value> = (0..20)
            .map(|i| {
                serde_json::json!({
                    "clientState": state,
                    "resource": format!("chats('19:r{round}c{i}@thread.v2')/messages('1')"),
                })
            })
            .collect();
        f.backend.send(Command::PushBody(
            serde_json::json!({ "value": items }).to_string(),
        ));
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let fetched = f
        .server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| {
            r.method.as_str() == "GET"
                && r.url.path().starts_with("/v1.0/chats/19:r")
                && r.url.path().ends_with("/messages")
        })
        .count();
    assert!((20..=26).contains(&fetched), "{fetched} fetches");
}
