//! Presence dots: the chat list's one-on-one contacts get their presence
//! fetched when the list is looked at, the push session follows them in one
//! bulk presence subscription (with the person's own presence), and a
//! contact's presence notification fetches that contact, batched.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chaat::auth::session::AccountRecord;
use chaat::backend::{AuthStatus, Backend, BackendOptions, Command, Event, Waker};
use chaat::paths::AppDirs;
use chaat::realtime::PushStatus;
use chaat::secrets::{MemoryStore, SecretStore};
use chaat::sync::{Resource, Trigger};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT: &str = "6731de76-14a6-49ae-97bc-6eba6914391e";
const ACCOUNT: &str = "u-me.t-1";
const SCOPES: &str = "User.Read Chat.ReadWrite ChatMessage.Send Presence.Read.All";

fn wait_for<T>(backend: &Backend, seconds: u64, mut pick: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        for event in backend.poll() {
            if let Some(found) = pick(event) {
                return found;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for an event");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The ids each `getPresencesByUserId` request asked for, in order.
async fn presence_requests(server: &MockServer) -> Vec<Vec<String>> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/v1.0/communications/getPresencesByUserId")
        .map(|r| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            body["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect()
        })
        .collect()
}

/// The resource of each presence subscription request, in order.
async fn presence_subscriptions(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/v1.0/subscriptions")
        .map(|r| serde_json::from_slice::<serde_json::Value>(&r.body).unwrap())
        .filter_map(|body| body["resource"].as_str().map(str::to_owned))
        .filter(|resource| resource.contains("presences"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn contacts_presence_is_fetched_on_look_and_followed_by_push() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(chaat_relay::serve(
        listener,
        chaat_relay::router(chaat_relay::Relay::default()),
    ));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access", "refresh_token": "refresh-2", "expires_in": 3600,
            "scope": SCOPES
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
            {"id": "19:ada", "chatType": "oneOnOne", "lastUpdatedDateTime": "2026-10-11T10:00:00Z",
             "members": [
                {"userId": "u-ada", "displayName": "Ada Lovelace"},
                {"userId": "u-me", "displayName": "Alex Rivera"}
             ]},
            {"id": "19:team", "chatType": "group", "topic": "Launch", "lastUpdatedDateTime": "2026-10-11T09:00:00Z",
             "members": [
                {"userId": "u-grace", "displayName": "Grace Hopper"},
                {"userId": "u-bo", "displayName": "Bo"},
                {"userId": "u-me", "displayName": "Alex Rivera"}
             ]}
        ]})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1.0/communications/getPresencesByUserId"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                {"id": "u-ada", "availability": "Busy", "activity": "InACall"}
            ]})),
        )
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
        granted: SCOPES.into(),
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
        },
    );
    backend.send(Command::Start {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        account: Some(ACCOUNT.into()),
    });
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Auth(AuthStatus::SignedIn { .. })).then_some(())
    });

    // Looking at the chat list (start) fetches the one-on-one contact's
    // presence; group members are left to the chat when it opens.
    backend.send(Command::Fetch {
        resource: Resource::Chats,
        trigger: Trigger::Start,
    });
    let presence = wait_for(&backend, 5, |e| match e {
        Event::Presence(list) => Some(list),
        _ => None,
    });
    assert_eq!(presence[0].0, "u-ada");
    assert_eq!(
        presence_requests(&server).await,
        vec![vec!["u-ada".to_owned()]]
    );

    // Push: one presence subscription, the person first, then the contact.
    backend.send(Command::Relay(Some(relay_url.clone())));
    wait_for(&backend, 10, |e| match e {
        Event::Push(PushStatus::Live) => Some(()),
        Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
        _ => None,
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let bulk = "communications/presences?$filter=id in ('u-me','u-ada')";
    while !presence_subscriptions(&server)
        .await
        .iter()
        .any(|r| r == bulk)
    {
        assert!(
            Instant::now() < deadline,
            "no bulk presence subscription: {:?}",
            presence_subscriptions(&server).await
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let body = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .rev()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/v1.0/subscriptions"
                && String::from_utf8_lossy(&r.body).contains("presences?")
        })
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body.body).unwrap();
    assert_eq!(body["includeResourceData"], false);
    let notification_url = body["notificationUrl"].as_str().unwrap().to_owned();
    let client_state = body["clientState"].as_str().unwrap().to_owned();

    // A burst of notices for the contact is one fetch for that contact.
    let fetches_before = presence_requests(&server).await.len();
    let notice = serde_json::json!({"value": [
        {"clientState": client_state, "changeType": "updated",
         "resource": "communications/presences/u-ada", "resourceData": {"id": "u-ada"}},
        {"clientState": client_state, "changeType": "updated",
         "resource": "communications/presences/u-ada", "resourceData": {"id": "u-ada"}}
    ]});
    let http = reqwest::Client::new();
    for _ in 0..2 {
        let status = http
            .post(&notification_url)
            .json(&notice)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 202);
    }
    let presence = wait_for(&backend, 5, |e| match e {
        Event::Presence(list) => Some(list),
        _ => None,
    });
    assert_eq!(presence[0].0, "u-ada");
    let fetches = presence_requests(&server).await;
    assert_eq!(fetches.len(), fetches_before + 1, "one fetch for the burst");
    assert_eq!(fetches.last().unwrap(), &vec!["u-ada".to_owned()]);

    backend.send(Command::Relay(None));
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });
}
