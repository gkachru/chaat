//! Worker handling of hide, mark unread, pins, status expiry and meeting-ID
//! lookup, against a local stand-in for Entra ID and Graph.

use std::time::{Duration, Instant};

use chaat::backend::{AccountCommand, AccountEvent};
use chaat::sync::{Resource, Trigger};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::{cached_chats, id_token, signed_in, wait_for};

const CHAT: &str = "19:ada-me@unq.gbl.spaces";

async fn mock_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "id_token": id_token(),
            "expires_in": 3600,
            "scope": "User.Read Chat.ReadWrite ChatMessage.Send Presence.ReadWrite OnlineMeetings.Read"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-2", "refresh_token": "refresh-2", "expires_in": 3600,
            "scope": "User.Read Chat.ReadWrite ChatMessage.Send"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/chats"))
        .and(header("authorization", "Bearer access-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [{
                "id": CHAT, "chatType": "oneOnOne", "lastUpdatedDateTime": "2026-10-11T10:00:00Z",
                "members": [
                    {"userId": "u-ada", "displayName": "Ada Lovelace", "email": "ada@contoso.com"},
                    {"userId": "u-me", "displayName": "Alex Rivera", "email": "alex@contoso.com"}
                ]
            }]})),
        )
        .mount(&server)
        .await;
    server
}

fn pins_path() -> String {
    format!("/v1.0/chats/{CHAT}/pinnedMessages")
}

fn pin_json(pin: &str, message: &str, text: &str) -> serde_json::Value {
    serde_json::json!({"id": pin, "message": {
        "id": message, "createdDateTime": "2026-11-20T10:00:00Z",
        "from": {"user": {"displayName": "Ada"}},
        "body": {"contentType": "html", "content": format!("<p>{text}</p>")}
    }})
}

fn error_body() -> serde_json::Value {
    serde_json::json!({"error": {"code": "Forbidden", "message": "no"}})
}

async fn requests_to(server: &MockServer, url: &str, verb: &str) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == url && r.method.as_str() == verb)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn hide_success_emits_and_updates_cache() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path(format!("/v1.0/chats/{CHAT}/hideForUser")))
        .and(body_string_contains("u-me"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(AccountCommand::HideChat {
        chat: CHAT.into(),
        hide: true,
    });
    let (chat, hidden, ok) = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::ChatHidden { chat, hidden, ok } => Some((chat, hidden, ok)),
        _ => None,
    });
    assert_eq!((chat.as_str(), hidden, ok), (CHAT, true, true));
    std::thread::sleep(Duration::from_millis(200));
    let chats = cached_chats(session);
    assert!(chats[0].hidden);
}

#[tokio::test(flavor = "multi_thread")]
async fn hide_failure_emits_not_ok() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path(format!("/v1.0/chats/{CHAT}/unhideForUser")))
        .respond_with(ResponseTemplate::new(403).set_body_json(error_body()))
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(AccountCommand::HideChat {
        chat: CHAT.into(),
        hide: false,
    });
    let (hidden, ok) = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::ChatHidden { hidden, ok, .. } => Some((hidden, ok)),
        _ => None,
    });
    assert_eq!((hidden, ok), (false, false));
    assert!(!cached_chats(session)[0].hidden);
}

#[tokio::test(flavor = "multi_thread")]
async fn mark_unread_failure_emits_event() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path(format!("/v1.0/chats/{CHAT}/markChatUnreadForUser")))
        .respond_with(ResponseTemplate::new(500).set_body_json(error_body()))
        .expect(1)
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session
        .backend
        .send(AccountCommand::MarkChatUnread(CHAT.into()));
    let chat = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::MarkUnreadFailed(chat) => Some(chat),
        _ => None,
    });
    assert_eq!(chat, CHAT);
}

#[tokio::test(flavor = "multi_thread")]
async fn pins_fetch_emits_pins() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path(pins_path()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"value": [pin_json("p1", "m1", "Ship it")]})),
        )
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(AccountCommand::Fetch {
        resource: Resource::Pins(CHAT.into()),
        trigger: Trigger::Opened,
    });
    let pins = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::Pins { chat, pins } if chat == CHAT => Some(pins),
        _ => None,
    });
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].id, "p1");
    assert_eq!(pins[0].message_id, "m1");
    assert_eq!(pins[0].author, "Ada");
    assert_eq!(pins[0].preview, "Ship it");
}

#[tokio::test(flavor = "multi_thread")]
async fn pin_then_refetches_pins() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path(pins_path()))
        .and(body_string_contains("messages/m1"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": "p1"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(pins_path()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"value": [pin_json("p1", "m1", "Ship it")]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(AccountCommand::PinMessage {
        chat: CHAT.into(),
        message: "m1".into(),
    });
    let pins = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::Pins { chat, pins } if chat == CHAT => Some(pins),
        AccountEvent::Error(text) => panic!("unexpected error: {text}"),
        _ => None,
    });
    assert_eq!(pins[0].message_id, "m1");
}

#[tokio::test(flavor = "multi_thread")]
async fn unpin_failure_errors_and_refetches_pins() {
    let server = mock_server().await;
    Mock::given(method("DELETE"))
        .and(path(format!("{}/p1", pins_path())))
        .respond_with(ResponseTemplate::new(403).set_body_json(error_body()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(pins_path()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"value": [pin_json("p1", "m1", "Ship it")]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(AccountCommand::UnpinMessage {
        chat: CHAT.into(),
        pin: "p1".into(),
    });
    let text = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::Error(text) => Some(text),
        _ => None,
    });
    assert!(text.starts_with("Unpinning"), "{text}");
    // The banner corrects itself: the pin that still exists comes back.
    let pins = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::Pins { pins, .. } => Some(pins),
        _ => None,
    });
    assert_eq!(pins.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn status_expiry_forwarded() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1.0/users/u-me/presence/setStatusMessage"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/presence"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "availability": "Available", "activity": "Available"
        })))
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(AccountCommand::SetStatusMessage {
        text: "Heads down".into(),
        expires: Some(0),
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let posted = loop {
        let found = requests_to(
            &server,
            "/v1.0/users/u-me/presence/setStatusMessage",
            "POST",
        )
        .await;
        if let Some(request) = found.into_iter().next() {
            break request;
        }
        assert!(Instant::now() < deadline, "no status request was sent");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let body: serde_json::Value = serde_json::from_slice(&posted.body).unwrap();
    assert_eq!(
        body["statusMessage"]["expiryDateTime"],
        serde_json::json!({"dateTime": "1970-01-01T00:00:00", "timeZone": "UTC"})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn meeting_by_id_emits_join_url_and_none() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/onlineMeetings"))
        .and(query_param(
            "$filter",
            "joinMeetingIdSettings/joinMeetingId eq '123456789012'",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                {"joinWebUrl": "https://teams.microsoft.com/l/meetup-join/abc"}
            ]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/onlineMeetings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": []})))
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session
        .backend
        .send(AccountCommand::FindMeetingById("123456789012".into()));
    let (id, url) = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::MeetingById { id, join_url } => Some((id, join_url)),
        _ => None,
    });
    assert_eq!(id, "123456789012");
    assert_eq!(
        url.as_deref(),
        Some("https://teams.microsoft.com/l/meetup-join/abc")
    );
    session
        .backend
        .send(AccountCommand::FindMeetingById("999999999".into()));
    let (id, url) = wait_for(&session.backend, 10, |e| match e {
        AccountEvent::MeetingById { id, join_url } => Some((id, join_url)),
        _ => None,
    });
    assert_eq!((id.as_str(), url), ("999999999", None));
}
