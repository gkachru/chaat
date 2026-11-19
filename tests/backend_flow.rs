//! The whole backend against local stand-ins for Entra ID and Graph: sign in
//! through the loopback redirect, list chats, read and send messages,
//! restore the session and cache on restart, and sign out.
//!
//! Only Microsoft's servers are simulated; everything else is the real code
//! path (the runtime thread, the worker, PKCE, the token exchange, the
//! Graph client, the SQLite cache).

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use chaat::backend::{AuthStatus, Backend, BackendOptions, Command, Event, Waker};
use chaat::model::ConversationId;
use chaat::paths::AppDirs;
use chaat::secrets::{MemoryStore, SecretStore};
use chaat::sync::{Resource, Trigger};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT: &str = "6731de76-14a6-49ae-97bc-6eba6914391e";
const CHAT: &str = "19:ada-me@unq.gbl.spaces";

fn id_token() -> String {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        br#"{"oid":"u-me","tid":"t-1","name":"Alex Rivera","preferred_username":"alex@contoso.com"}"#,
    );
    format!("eyJhbGciOiJub25lIn0.{payload}.")
}

// Events not yet looked at, so a wait never drops the rest of a batch.
thread_local! {
    /// Per test thread: tests run in parallel and must not steal events.
    static PENDING: std::cell::RefCell<Vec<Event>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Waits for the first event `pick` accepts, failing after `seconds`.
/// Events before it are skipped; events after it stay for the next wait.
fn wait_for<T>(backend: &Backend, seconds: u64, mut pick: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let mut pending = PENDING.take();
        pending.extend(backend.poll());
        while !pending.is_empty() {
            let event = pending.remove(0);
            if std::env::var("CHAAT_TEST_EVENTS").is_ok() {
                eprintln!("event: {event:?}");
            }
            if let Some(found) = pick(event) {
                PENDING.set(pending);
                return found;
            }
        }
        PENDING.set(pending);
        assert!(Instant::now() < deadline, "timed out waiting for an event");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn query_value(url: &str, key: &str) -> String {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or_default();
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .map(|value| urlencoding::decode(value).unwrap().into_owned())
        .unwrap_or_default()
}

async fn mock_microsoft() -> MockServer {
    mock_server(true).await
}

/// `sends`: whether the test sends the one message the fixture expects.
async fn mock_server(sends: bool) -> MockServer {
    let server = MockServer::start().await;
    // Entra: the code Chaat's loopback received, with the PKCE verifier.
    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=the-code"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "id_token": id_token(),
            "expires_in": 3600,
            "scope": "User.Read Chat.ReadWrite ChatMessage.Send Team.ReadBasic.All Channel.ReadBasic.All ChannelMessage.Send Presence.Read.All Calendars.Read"
        })))
        .mount(&server)
        .await;
    // Entra: the stored refresh token, after a restart.
    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=refresh-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-2", "refresh_token": "refresh-2", "expires_in": 3600,
            "scope": "User.Read Chat.ReadWrite ChatMessage.Send"
        })))
        .mount(&server)
        .await;
    // Graph.
    Mock::given(method("GET"))
        .and(path("/v1.0/me/chats"))
        .and(header("authorization", "Bearer access-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [{
            "id": CHAT, "chatType": "oneOnOne", "lastUpdatedDateTime": "2026-10-11T10:00:00Z",
            "members": [
                {"userId": "u-ada", "displayName": "Ada Lovelace", "email": "ada@contoso.com"},
                {"userId": "u-me", "displayName": "Alex Rivera", "email": "alex@contoso.com"}
            ],
            "lastMessagePreview": {"id": "1700", "createdDateTime": "2026-10-11T10:00:00Z", "messageType": "message",
                "body": {"contentType": "html", "content": "<p>Ship it?</p>"},
                "from": {"user": {"id": "u-ada", "displayName": "Ada Lovelace"}}},
            "viewpoint": {"lastMessageReadDateTime": "2026-10-11T09:00:00Z"}
        }]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v1.0/chats/{CHAT}/messages")))
        .and(query_param("$orderby", "createdDateTime desc"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                {"id": "1700", "messageType": "message", "createdDateTime": "2026-10-11T10:00:00Z",
                 "lastModifiedDateTime": "2026-10-11T10:00:00Z",
                 "from": {"user": {"id": "u-ada", "displayName": "Ada Lovelace"}},
                 "body": {"contentType": "html", "content": "<p>Ship <b>it</b>?</p>"}},
                {"id": "1690", "messageType": "message", "createdDateTime": "2026-10-11T09:50:00Z",
                 "lastModifiedDateTime": "2026-10-11T09:50:00Z",
                 "from": {"user": {"id": "u-me", "displayName": "Alex Rivera"}},
                 "body": {"contentType": "html", "content": "<p>Build is green</p>"}}
            ]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/v1.0/chats/{CHAT}/messages")))
        .and(body_string_contains("<strong>today</strong>"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": "1701", "messageType": "message", "createdDateTime": "2026-10-11T10:01:00Z",
            "lastModifiedDateTime": "2026-10-11T10:01:00Z",
            "from": {"user": {"id": "u-me", "displayName": "Alex Rivera"}},
            "body": {"contentType": "html", "content": "<p>Shipping <strong>today</strong></p>"}
        })))
        .expect(if sends { 1..=1 } else { 0..=u64::MAX })
        .mount(&server)
        .await;
    // A restored conversation refreshes by modification date, independently
    // of the newest-page request above. No messages changed in this fixture.
    Mock::given(method("GET"))
        .and(path(format!("/v1.0/chats/{CHAT}/messages")))
        .and(query_param("$orderby", "lastModifiedDateTime desc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": []})))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_sync_send_restore_and_sign_out() {
    let server = mock_microsoft().await;
    let dir = tempfile::tempdir().unwrap();
    let dirs = AppDirs::under(dir.path());
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let options = BackendOptions {
        authority: server.uri(),
        graph_base: format!("{}/v1.0", server.uri()),
        secrets: Arc::clone(&secrets),
        open_browser: false,
        push_renew_every: chaat::realtime::RENEW_EVERY,
        relay_connect_timeout: std::time::Duration::from_secs(20),
    };

    // First run: no account yet.
    let backend = Backend::spawn_with(dirs.clone(), Waker::default(), options.clone());
    backend.send(Command::Start {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        account: None,
    });
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Auth(AuthStatus::SignedOut)).then_some(())
    });

    // Sign in: play the browser's part by following the redirect.
    backend.send(Command::SignIn {
        extra_scopes: Vec::new(),
        device_code: false,
    });
    let url = wait_for(&backend, 5, |e| match e {
        Event::Auth(AuthStatus::WaitingForBrowser { url }) => Some(url),
        _ => None,
    });
    assert!(url.starts_with(&format!(
        "{}/organizations/oauth2/v2.0/authorize?",
        server.uri()
    )));
    assert_eq!(query_value(&url, "code_challenge_method"), "S256");
    let redirect = query_value(&url, "redirect_uri");
    let state = query_value(&url, "state");
    let port = redirect.trim_start_matches("http://localhost:");
    let landing = reqwest::get(format!(
        "http://127.0.0.1:{port}/?code=the-code&state={state}"
    ))
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
    assert!(landing.contains("signed in"));
    let account = wait_for(&backend, 10, |e| match e {
        Event::Auth(AuthStatus::SignedIn { account, name, .. }) => {
            assert_eq!(name, "Alex Rivera");
            Some(account)
        }
        Event::Auth(AuthStatus::Failed(reason)) => panic!("sign-in failed: {reason}"),
        _ => None,
    });
    assert_eq!(account, "u-me.t-1");
    assert_eq!(
        secrets
            .read(&chaat::secrets::refresh_key(&account))
            .unwrap()
            .as_deref(),
        Some("refresh-1")
    );

    // Chats, then the chat's messages, then a send.
    backend.send(Command::Fetch {
        resource: Resource::Chats,
        trigger: Trigger::Start,
    });
    let chats = wait_for(&backend, 10, |e| match e {
        Event::Chats {
            chats,
            cached: false,
        } => Some(chats),
        _ => None,
    });
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].title("u-me"), "Ada Lovelace");
    assert!(chats[0].is_unread("u-me"));

    let conversation = ConversationId::Chat(CHAT.into());
    backend.send(Command::Fetch {
        resource: Resource::Messages(conversation.clone()),
        trigger: Trigger::Opened,
    });
    let messages = wait_for(&backend, 10, |e| match e {
        Event::Messages {
            messages,
            cached: false,
            ..
        } => Some(messages),
        _ => None,
    });
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].body.to_plain(), "Ship it?");

    backend.send(Command::Send {
        conversation: conversation.clone(),
        local_id: "local-1".into(),
        html: chaat::html::compose("Shipping **today**"),
        reply_to: None,
        mentions: Vec::new(),
        files: Vec::new(),
        images: Vec::new(),
        recipients: Vec::new(),
    });
    let sent = wait_for(&backend, 10, |e| match e {
        Event::Sent {
            local_id, message, ..
        } => Some((local_id, message)),
        Event::SendFailed { reason, .. } => panic!("send failed: {reason}"),
        _ => None,
    });
    assert_eq!(sent.0, "local-1");
    assert_eq!(sent.1.id, "1701");
    drop(backend);
    PENDING.with_borrow_mut(Vec::clear);

    // Second run: the session comes back from the credential store, and the
    // cache is shown before any request.
    let backend = Backend::spawn_with(dirs.clone(), Waker::default(), options.clone());
    backend.send(Command::Start {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        account: Some(account.clone()),
    });
    let cached = wait_for(&backend, 5, |e| match e {
        Event::Chats {
            chats,
            cached: true,
        } => Some(chats),
        _ => None,
    });
    assert_eq!(cached.len(), 1);
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Auth(AuthStatus::SignedIn { .. })).then_some(())
    });
    backend.send(Command::Fetch {
        resource: Resource::Messages(conversation.clone()),
        trigger: Trigger::Opened,
    });
    let cached_messages = wait_for(&backend, 5, |e| match e {
        Event::Messages {
            messages,
            cached: true,
            more: Some(true),
            ..
        } => Some(messages),
        _ => None,
    });
    assert_eq!(cached_messages.len(), 3, "two fetched and one sent");
    // Opening serves the cache before its Graph refresh completes. Wait for
    // that refresh so its requests cannot be mistaken for a cache-only reopen.
    wait_for(&backend, 5, |e| match e {
        Event::Messages {
            conversation: fetched,
            cached: false,
            ..
        } if fetched == conversation => Some(()),
        _ => None,
    });
    // RAM eviction can reopen a conversation without claiming a Graph fetch.
    let requests_before = server.received_requests().await.unwrap().len();
    backend.send(Command::CachedMessages(conversation.clone()));
    let reopened = wait_for(&backend, 5, |e| match e {
        Event::Messages {
            messages,
            cached: true,
            ..
        } => Some(messages),
        _ => None,
    });
    assert_eq!(reopened.len(), 3);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        requests_before
    );
    let before = cached_messages.iter().map(|m| m.created).min().unwrap();
    Mock::given(method("GET"))
        .and(path(format!("/v1.0/chats/{CHAT}/messages")))
        .and(query_param(
            "$filter",
            format!("createdDateTime lt {}", chaat::time::to_iso(before)),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [{
                "id": "1600", "messageType": "message", "createdDateTime": "2026-10-10T09:00:00Z",
                "body": {"contentType": "text", "content": "Earlier history"}
            }]})),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    backend.send(Command::LoadOlder {
        conversation: conversation.clone(),
        before: Some(before),
    });
    let earlier = wait_for(&backend, 5, |e| match e {
        Event::Messages {
            messages,
            merge: chaat::backend::Merge::Older,
            more: Some(false),
            cached: false,
            ..
        } => Some(messages),
        _ => None,
    });
    assert_eq!(
        earlier[0].id, "1600",
        "reopened cached chats can load yesterday's predecessors"
    );
    // The refresh token was used and rotated by the date-ranged refresh.
    let deadline = Instant::now() + Duration::from_secs(5);
    while secrets
        .read(&chaat::secrets::refresh_key(&account))
        .unwrap()
        .as_deref()
        != Some("refresh-2")
    {
        assert!(
            Instant::now() < deadline,
            "the rotated refresh token was not stored"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // Reopening an evicted image uses the account's encoded cache, with no
    // second Graph download. This remains available after RAM eviction.
    Mock::given(method("GET"))
        .and(path("/v1.0/hosted-image"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"image fixture".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    let media_url = format!("{}/v1.0/hosted-image", server.uri());
    for _ in 0..2 {
        backend.send(Command::Media(media_url.clone()));
        let bytes = wait_for(&backend, 5, |e| match e {
            Event::Media { url, bytes } if url == media_url => Some(bytes),
            _ => None,
        });
        assert_eq!(bytes, Some(b"image fixture".to_vec()));
    }

    // Sign out: the token and the cache are gone.
    backend.send(Command::SignOut);
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Auth(AuthStatus::SignedOut)).then_some(())
    });
    assert_eq!(
        secrets
            .read(&chaat::secrets::refresh_key(&account))
            .unwrap(),
        None
    );
    assert!(!dirs.account(&account).exists());
}

// -- People panel -------------------------------------------------------------

struct Session {
    backend: Backend,
    dirs: AppDirs,
    options: BackendOptions,
    account: String,
    _dir: tempfile::TempDir,
}

/// Signs in against the mock and loads the chat list, so the cache holds
/// the chat.
async fn signed_in(server: &MockServer) -> Session {
    let dir = tempfile::tempdir().unwrap();
    let dirs = AppDirs::under(dir.path());
    let options = BackendOptions {
        authority: server.uri(),
        graph_base: format!("{}/v1.0", server.uri()),
        secrets: Arc::new(MemoryStore::default()),
        open_browser: false,
        push_renew_every: chaat::realtime::RENEW_EVERY,
        relay_connect_timeout: std::time::Duration::from_secs(20),
    };
    PENDING.with_borrow_mut(Vec::clear);
    let backend = Backend::spawn_with(dirs.clone(), Waker::default(), options.clone());
    backend.send(Command::Start {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        account: None,
    });
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Auth(AuthStatus::SignedOut)).then_some(())
    });
    backend.send(Command::SignIn {
        extra_scopes: Vec::new(),
        device_code: false,
    });
    let url = wait_for(&backend, 5, |e| match e {
        Event::Auth(AuthStatus::WaitingForBrowser { url }) => Some(url),
        _ => None,
    });
    let redirect = query_value(&url, "redirect_uri");
    let state = query_value(&url, "state");
    let port = redirect.trim_start_matches("http://localhost:");
    reqwest::get(format!(
        "http://127.0.0.1:{port}/?code=the-code&state={state}"
    ))
    .await
    .unwrap();
    let account = wait_for(&backend, 10, |e| match e {
        Event::Auth(AuthStatus::SignedIn { account, .. }) => Some(account),
        _ => None,
    });
    backend.send(Command::Fetch {
        resource: Resource::Chats,
        trigger: Trigger::Start,
    });
    wait_for(&backend, 10, |e| {
        matches!(e, Event::Chats { cached: false, .. }).then_some(())
    });
    Session {
        backend,
        dirs,
        options,
        account,
        _dir: dir,
    }
}

/// Restarts the backend on the same files and returns the cached chats.
fn cached_chats(session: Session) -> Vec<chaat::model::Chat> {
    let Session {
        backend,
        dirs,
        options,
        account,
        _dir,
    } = session;
    drop(backend);
    PENDING.with_borrow_mut(Vec::clear);
    let backend = Backend::spawn_with(dirs, Waker::default(), options);
    backend.send(Command::Start {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        account: Some(account),
    });
    wait_for(&backend, 5, |e| match e {
        Event::Chats {
            chats,
            cached: true,
        } => Some(chats),
        _ => None,
    })
}

fn members_path() -> String {
    format!("/v1.0/chats/{CHAT}/members")
}

fn member_json(membership: &str, user: &str, name: &str) -> serde_json::Value {
    serde_json::json!({"id": membership, "userId": user, "displayName": name})
}

#[tokio::test(flavor = "multi_thread")]
async fn members_fetch_sends_event_and_updates_store() {
    let server = mock_server(false).await;
    Mock::given(method("GET"))
        .and(path(members_path()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                member_json("m1", "u-me", "Alex Rivera"),
                member_json("m2", "u-ada", "Ada Lovelace"),
                member_json("m3", "u-grace", "Grace Hopper"),
            ]})),
        )
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(Command::Fetch {
        resource: Resource::Members(CHAT.into()),
        trigger: Trigger::Opened,
    });
    let members = wait_for(&session.backend, 10, |e| match e {
        Event::Members { chat, members } if chat == CHAT => Some(members),
        _ => None,
    });
    assert_eq!(members.len(), 3);
    assert_eq!(members[2].user_id, "u-grace");
    // The cache holds the full list.
    std::thread::sleep(Duration::from_millis(200));
    let chats = cached_chats(session);
    assert_eq!(chats[0].members.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn add_members_reports_partial_failure() {
    let server = mock_server(false).await;
    for (user, status) in [("u1", 201), ("u2", 500)] {
        Mock::given(method("POST"))
            .and(path(members_path()))
            .and(body_string_contains(format!("users('{user}')")))
            .respond_with(
                ResponseTemplate::new(status).set_body_json(serde_json::json!({"id": "x"})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(members_path()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                member_json("m1", "u-me", "Alex Rivera"),
                member_json("m4", "u1", "One"),
            ]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(Command::AddMembers {
        chat: CHAT.into(),
        users: vec!["u1".into(), "u2".into()],
        history: false,
    });
    let (added, failed) = wait_for(&session.backend, 20, |e| match e {
        Event::MembersAdded {
            chat,
            added,
            failed,
        } if chat == CHAT => Some((added, failed)),
        _ => None,
    });
    assert_eq!(added, vec!["u1".to_string()]);
    assert_eq!(failed, vec!["u2".to_string()]);
    // Then the members are read back.
    let members = wait_for(&session.backend, 10, |e| match e {
        Event::Members { chat, members } if chat == CHAT => Some(members),
        _ => None,
    });
    assert_eq!(members.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn leave_finds_my_membership_and_deletes() {
    let server = mock_server(false).await;
    Mock::given(method("GET"))
        .and(path(members_path()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                member_json("m-ada", "u-ada", "Ada Lovelace"),
                member_json("m#me=", "u-me", "Alex Rivera"),
            ]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{}/m%23me%3D", members_path())))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(Command::LeaveChat(CHAT.into()));
    wait_for(&session.backend, 10, |e| match e {
        Event::LeftChat(chat) if chat == CHAT => Some(()),
        Event::LeaveFailed { .. } => panic!("leave failed"),
        _ => None,
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(cached_chats(session).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn leave_403_reports_admin() {
    let server = mock_server(false).await;
    Mock::given(method("GET"))
        .and(path(members_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"value": [member_json("m-me", "u-me", "Alex Rivera")]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{}/m-me", members_path())))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(
                serde_json::json!({"error": {"code": "Forbidden", "message": "no"}}),
            ),
        )
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(Command::LeaveChat(CHAT.into()));
    let admin = wait_for(&session.backend, 10, |e| match e {
        Event::LeaveFailed { chat, admin } if chat == CHAT => Some(admin),
        _ => None,
    });
    assert!(admin);
    // The chat stays in the cache.
    assert_eq!(cached_chats(session).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn leave_without_my_membership_fails() {
    let server = mock_server(false).await;
    Mock::given(method("GET"))
        .and(path(members_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"value": [member_json("m-ada", "u-ada", "Ada Lovelace")]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;
    let session = signed_in(&server).await;
    session.backend.send(Command::LeaveChat(CHAT.into()));
    let admin = wait_for(&session.backend, 10, |e| match e {
        Event::LeaveFailed { chat, admin } if chat == CHAT => Some(admin),
        _ => None,
    });
    assert!(!admin);
}
