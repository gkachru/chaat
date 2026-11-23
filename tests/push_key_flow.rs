//! Push through a relay that requires an access key: the backend stores the
//! key in the credential store, sends it in a header (never the URL), and
//! says plainly when the relay refuses it.

use std::sync::Arc;
use std::time::Duration;

use chaat::auth::session::AccountRecord;
use chaat::backend::{
    AccountCommand, AccountEvent, AuthStatus, Backend, BackendOptions, ServiceCommand,
    ServiceEvent, ServiceRole, Waker,
};
use chaat::paths::AppDirs;
use chaat::realtime::{PushStatus, RelayKey};
use chaat::secrets::{MemoryStore, SecretStore};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::wait_for;

const CLIENT: &str = "6731de76-14a6-49ae-97bc-6eba6914391e";
const ACCOUNT: &str = "u-me.t-1";
const KEY: &str = "office-relay-key-0123456789";

fn push_result(event: AccountEvent) -> Option<Result<(), String>> {
    match event {
        AccountEvent::Push(PushStatus::Live) => Some(Ok(())),
        AccountEvent::Push(PushStatus::Failed(reason)) => Some(Err(reason)),
        _ => None,
    }
}

/// Waits for the first service event `pick` accepts.
fn wait_service<T>(
    service: &Backend<ServiceRole>,
    seconds: u64,
    mut pick: impl FnMut(ServiceEvent) -> Option<T>,
) -> T {
    let deadline = Duration::from_secs(seconds);
    let start = std::time::Instant::now();
    loop {
        for event in service.poll() {
            if let Some(found) = pick(event) {
                return found;
            }
        }
        assert!(
            start.elapsed() < deadline,
            "timed out waiting for a service event"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keyed_relay_needs_the_stored_key() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let relay = chaat_relay::Relay::default().with_key(KEY).unwrap();
    tokio::spawn(chaat_relay::serve(listener, chaat_relay::router(relay)));

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
        // Slower than a quick quit, as Graph often is.
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_millis(400)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": []})))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let dirs = AppDirs::under(dir.path());
    let store = Arc::new(MemoryStore::default());
    let secrets: Arc<dyn SecretStore> = store.clone();
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

    let options = BackendOptions {
        authority: server.uri(),
        graph_base: format!("{}/v1.0", server.uri()),
        secrets,
        open_browser: false,
        push_renew_every: chaat::realtime::RENEW_EVERY,
        relay_connect_timeout: std::time::Duration::from_secs(20),
    };
    let backend: Backend = Backend::spawn_with(dirs.clone(), Waker::default(), options.clone());
    // The relay key belongs to the service backend, as in the app.
    let service: Backend<ServiceRole> = Backend::spawn_with(dirs, Waker::default(), options);
    backend.send(AccountCommand::Start {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        account: Some(ACCOUNT.into()),
    });
    wait_for(&backend, 5, |e| {
        matches!(e, AccountEvent::Auth(AuthStatus::SignedIn { .. })).then_some(())
    });

    // No key stored: the relay refuses, and the person is told why.
    backend.send(AccountCommand::Relay(Some(relay_url.clone())));
    assert_eq!(
        wait_for(&backend, 10, push_result),
        Err("the relay needs an access key".to_owned())
    );

    // A wrong key is refused too.
    service.send(ServiceCommand::SaveRelayKey(Some(RelayKey(
        "not-the-office-key-000".into(),
    ))));
    assert_eq!(
        wait_service(&service, 5, |e| match e {
            ServiceEvent::RelayKeySaved { set, ok } => Some((set, ok)),
            _ => None,
        }),
        (true, true)
    );
    backend.send(AccountCommand::Relay(Some(relay_url.clone())));
    assert_eq!(
        wait_for(&backend, 10, push_result),
        Err("the relay did not accept the access key".to_owned())
    );

    // The right key goes to the credential store and opens the relay.
    service.send(ServiceCommand::SaveRelayKey(Some(RelayKey(KEY.into()))));
    wait_service(&service, 5, |e| {
        matches!(
            e,
            ServiceEvent::RelayKeySaved {
                set: true,
                ok: true
            }
        )
        .then_some(())
    });
    assert_eq!(
        store.read(chaat::secrets::RELAY_KEY).unwrap().as_deref(),
        Some(KEY)
    );
    backend.send(AccountCommand::Relay(Some(relay_url.clone())));
    assert_eq!(wait_for(&backend, 10, push_result), Ok(()));

    // The key never reaches Graph: it is not part of any URL Chaat registers.
    for request in server.received_requests().await.unwrap() {
        assert!(!String::from_utf8_lossy(&request.body).contains(KEY));
        assert!(!request.url.as_str().contains(KEY));
    }

    // Removing the key deletes it from the store.
    service.send(ServiceCommand::SaveRelayKey(None));
    wait_service(&service, 5, |e| {
        matches!(
            e,
            ServiceEvent::RelayKeySaved {
                set: false,
                ok: true
            }
        )
        .then_some(())
    });
    assert_eq!(store.read(chaat::secrets::RELAY_KEY).unwrap(), None);

    // Quitting while push is live deletes the subscriptions before the
    // backend ends, even when Graph takes its time.
    let before = deletes(&server).await;
    let leftovers = AppDirs::under(dir.path())
        .account(ACCOUNT)
        .join(chaat::realtime::LEFTOVERS_FILE);
    assert!(leftovers.exists(), "live subscriptions are recorded");
    drop(backend);
    assert!(
        !leftovers.exists(),
        "the deletes finished before the backend ended"
    );
    assert!(
        deletes(&server).await >= before + 2,
        "chat and presence subscriptions deleted at shutdown"
    );
}

async fn deletes(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "DELETE" && r.url.path() == "/v1.0/subscriptions/sub-1")
        .count()
}
