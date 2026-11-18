//! Presence dots: the chat list's one-on-one contacts and group members get
//! their presence fetched when the list is looked at, the push session
//! follows them in one bulk presence subscription (with the person's own
//! presence), and presence notifications fetch those people, batched.

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

/// A relay, Graph with a one-on-one chat (Ada) and a group (Grace, Bo), and
/// a signed-in backend. Callers may mount higher-priority mocks first.
async fn start(server: &MockServer) -> (Backend, String, tempfile::TempDir) {
    start_with(
        server,
        chaat::realtime::RENEW_EVERY,
        Duration::from_secs(20),
    )
    .await
}

/// [`start`], renewing push subscriptions every `renew`.
async fn start_with(
    server: &MockServer,
    renew: Duration,
    connect: Duration,
) -> (Backend, String, tempfile::TempDir) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(chaat_relay::serve(
        listener,
        chaat_relay::router(chaat_relay::Relay::default()),
    ));

    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access", "refresh_token": "refresh-2", "expires_in": 3600,
            "scope": SCOPES
        })))
        .mount(server)
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
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1.0/communications/getPresencesByUserId"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                {"id": "u-ada", "availability": "Busy", "activity": "InACall"}
            ]})),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1.0/subscriptions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": "sub-1", "expirationDateTime": "2026-10-11T13:00:00Z"
        })))
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1.0/subscriptions/sub-1"))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
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
            push_renew_every: renew,
            relay_connect_timeout: connect,
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
    (backend, relay_url, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn contacts_presence_is_fetched_on_look_and_followed_by_push() {
    let server = MockServer::start().await;
    let (backend, relay_url, _dir) = start(&server).await;

    // Looking at the chat list (start) fetches presence for the one-on-one
    // contact, then the group's members, in one request.
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
        vec![vec![
            "u-ada".to_owned(),
            "u-grace".to_owned(),
            "u-bo".to_owned()
        ]]
    );

    // Push: one presence subscription: the person, the contact, the group.
    backend.send(Command::Relay(Some(relay_url.clone())));
    wait_for(&backend, 10, |e| match e {
        Event::Push(PushStatus::Live) => Some(()),
        Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
        _ => None,
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let bulk = "communications/presences?$filter=id in ('u-me','u-ada','u-grace','u-bo')";
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

    // A burst of notices, for the contact and a group member, is one fetch
    // for both.
    let fetches_before = presence_requests(&server).await.len();
    let notice = serde_json::json!({"value": [
        {"clientState": client_state, "changeType": "updated",
         "resource": "communications/presences/u-ada", "resourceData": {"id": "u-ada"}},
        {"clientState": client_state, "changeType": "updated",
         "resource": "communications/presences/u-grace", "resourceData": {"id": "u-grace"}}
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
    let mut last = fetches.last().unwrap().clone();
    last.sort();
    assert_eq!(last, ["u-ada", "u-grace"]);

    backend.send(Command::Relay(None));
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });
}

/// Presence subscriptions for more than `max` people.
struct MoreThan(usize);

impl wiremock::Match for MoreThan {
    fn matches(&self, request: &wiremock::Request) -> bool {
        let Ok(body) = serde_json::from_slice::<serde_json::Value>(&request.body) else {
            return false;
        };
        let resource = body["resource"].as_str().unwrap_or_default();
        resource.contains("presences") && resource.matches('\'').count() / 2 > self.0
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_presence_list_shrinks_and_is_not_retried() {
    let server = MockServer::start().await;
    // Graph refuses presence subscriptions for more than two people.
    Mock::given(method("POST"))
        .and(path("/v1.0/subscriptions"))
        .and(MoreThan(2))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            serde_json::json!({"error": {"code": "ExtensionError", "message": "x"}}),
        ))
        .with_priority(1)
        .mount(&server)
        .await;
    let (backend, relay_url, _dir) = start(&server).await;
    backend.send(Command::Fetch {
        resource: Resource::Chats,
        trigger: Trigger::Start,
    });
    wait_for(&backend, 5, |e| match e {
        Event::Presence(list) => Some(list),
        _ => None,
    });
    backend.send(Command::Relay(Some(relay_url)));
    wait_for(&backend, 10, |e| match e {
        Event::Push(PushStatus::Live) => Some(()),
        Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
        _ => None,
    });
    // Four people refused, two accepted, three refused (after deleting the
    // two, since Graph allows one), then two again: the person and the
    // newest contact.
    let deadline = Instant::now() + Duration::from_secs(5);
    let two = "communications/presences?$filter=id in ('u-me','u-ada')";
    while !presence_subscriptions(&server)
        .await
        .iter()
        .any(|r| r == two)
    {
        assert!(
            Instant::now() < deadline,
            "no smaller presence subscription: {:?}",
            presence_subscriptions(&server).await
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let four = "communications/presences?$filter=id in ('u-me','u-ada','u-grace','u-bo')";
    let three = "communications/presences?$filter=id in ('u-me','u-ada','u-grace')";
    assert_eq!(
        presence_subscriptions(&server).await,
        [four, two, three, two]
    );
    // Later chat-list updates keep to the size that worked.
    let settled = presence_subscriptions(&server).await.len();
    for _ in 0..3 {
        backend.send(Command::Fetch {
            resource: Resource::Chats,
            trigger: Trigger::Refresh,
        });
        tokio::time::sleep(Duration::from_millis(2100)).await;
    }
    assert_eq!(
        presence_subscriptions(&server).await.len(),
        settled,
        "refused sizes are not retried: {:?}",
        presence_subscriptions(&server).await
    );
    backend.send(Command::Relay(None));
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn users_graph_names_are_left_out_and_stay_out() {
    let server = MockServer::start().await;
    // Graph cannot follow Grace, and says so, as it does on live tenants.
    Mock::given(method("POST"))
        .and(path("/v1.0/subscriptions"))
        .and(wiremock::matchers::body_string_contains("'u-grace'"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {"code": "ExtensionError", "message":
                "Operation: Create; Exception: [Status Code: BadRequest; Reason: Subscription failed for some users. Remove the failing users and retry. (u-grace)]"}
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    let (backend, relay_url, dir) = start(&server).await;
    backend.send(Command::Fetch {
        resource: Resource::Chats,
        trigger: Trigger::Start,
    });
    wait_for(&backend, 5, |e| match e {
        Event::Presence(list) => Some(list),
        _ => None,
    });
    backend.send(Command::Relay(Some(relay_url.clone())));
    wait_for(&backend, 10, |e| match e {
        Event::Push(PushStatus::Live) => Some(()),
        Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
        _ => None,
    });
    let four = "communications/presences?$filter=id in ('u-me','u-ada','u-grace','u-bo')";
    let without_grace = "communications/presences?$filter=id in ('u-me','u-ada','u-bo')";
    let deadline = Instant::now() + Duration::from_secs(5);
    while !presence_subscriptions(&server)
        .await
        .iter()
        .any(|r| r == without_grace)
    {
        assert!(
            Instant::now() < deadline,
            "{:?}",
            presence_subscriptions(&server).await
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(presence_subscriptions(&server).await, [four, without_grace]);
    // Later chat-list updates don't bring Grace back.
    for _ in 0..2 {
        backend.send(Command::Fetch {
            resource: Resource::Chats,
            trigger: Trigger::Refresh,
        });
        tokio::time::sleep(Duration::from_millis(2100)).await;
    }
    assert_eq!(presence_subscriptions(&server).await, [four, without_grace]);
    backend.send(Command::Relay(None));
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });

    // The refusal is remembered: the next session leaves Grace out at once.
    let refused = AppDirs::under(dir.path())
        .account(ACCOUNT)
        .join(chaat::realtime::REFUSED_FILE);
    assert!(chaat::realtime::read_refused(&refused, chaat::time::now()).contains_key("u-grace"));
    backend.send(Command::Relay(Some(relay_url)));
    wait_for(&backend, 10, |e| match e {
        Event::Push(PushStatus::Live) => Some(()),
        Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
        _ => None,
    });
    assert_eq!(
        presence_subscriptions(&server).await,
        [four, without_grace, without_grace],
        "no refused request in the new session"
    );
    backend.send(Command::Relay(None));
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_presence_conflict_is_tried_again_at_the_next_renewal() {
    let server = MockServer::start().await;
    // Another presence subscription holds the person's one place at first.
    Mock::given(method("POST"))
        .and(path("/v1.0/subscriptions"))
        .and(wiremock::matchers::body_string_contains("presences"))
        .respond_with(ResponseTemplate::new(409).set_body_json(
            serde_json::json!({"error": {"code": "ExtensionError", "message": "x"}}),
        ))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/v1.0/subscriptions/sub-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "sub-1", "expirationDateTime": "2026-10-11T14:00:00Z"
        })))
        .mount(&server)
        .await;
    let (backend, relay_url, _dir) =
        start_with(&server, Duration::from_secs(4), Duration::from_secs(20)).await;
    backend.send(Command::Relay(Some(relay_url)));
    wait_for(&backend, 15, |e| match e {
        Event::Push(PushStatus::Live) => Some(()),
        Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
        _ => None,
    });
    // Refused twice (once, then after a pause): push is live without it.
    assert_eq!(presence_subscriptions(&server).await.len(), 2);
    // The renewal tries once more, and Graph now accepts.
    let deadline = Instant::now() + Duration::from_secs(10);
    while presence_subscriptions(&server).await.len() < 3 {
        assert!(Instant::now() < deadline, "no retry at the renewal");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        presence_subscriptions(&server).await.len(),
        3,
        "one retry, no more"
    );
    backend.send(Command::Relay(None));
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_presence_subscription_in_the_way_is_found_and_deleted() {
    let server = MockServer::start().await;
    // A crashed run left a presence subscription holding the one place.
    Mock::given(method("POST"))
        .and(path("/v1.0/subscriptions"))
        .and(wiremock::matchers::body_string_contains("presences"))
        .respond_with(ResponseTemplate::new(409).set_body_json(
            serde_json::json!({"error": {"code": "ExtensionError", "message": "x"}}),
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/subscriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
            {"id": "sub-old-presence", "resource": "communications/presences?$filter=id in ('u-me')"},
            {"id": "sub-old-chats", "resource": "/users/u-me/chats/getAllMessages"}
        ]})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1.0/subscriptions/sub-old-presence"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v1.0/subscriptions/sub-old-chats"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;
    let (backend, relay_url, _dir) = start(&server).await;
    let started = Instant::now();
    backend.send(Command::Relay(Some(relay_url)));
    wait_for(&backend, 15, |e| match e {
        Event::Push(PushStatus::Live) => Some(()),
        Event::Push(PushStatus::Failed(reason)) => panic!("push failed: {reason}"),
        _ => None,
    });
    // Refused once, then the same list accepted at once after the blocker
    // went (the contacts' list may follow when the chat list arrives).
    let attempts = presence_subscriptions(&server).await;
    assert!(attempts.len() >= 2, "{attempts:?}");
    assert_eq!(attempts[0], attempts[1]);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "no pause when a blocker was deleted"
    );
    backend.send(Command::Relay(None));
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_never_answers_counts_as_unreachable() {
    let server = MockServer::start().await;
    let (backend, _relay_url, _dir) = start_with(
        &server,
        chaat::realtime::RENEW_EVERY,
        Duration::from_secs(1),
    )
    .await;
    // A "relay" that takes the connection and never says a word.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_url = format!("http://127.0.0.1:{}", silent.local_addr().unwrap().port());
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = silent.accept().await {
            held.push(socket);
        }
    });
    let started = Instant::now();
    backend.send(Command::Relay(Some(silent_url)));
    let reason = wait_for(&backend, 10, |e| match e {
        Event::Push(PushStatus::Failed(reason)) => Some(reason),
        Event::Push(PushStatus::Live) => panic!("a silent relay cannot be live"),
        _ => None,
    });
    assert_eq!(reason, "the relay did not answer in time");
    assert!(started.elapsed() < Duration::from_secs(5));
    backend.send(Command::Relay(None));
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Push(PushStatus::Off)).then_some(())
    });
}
