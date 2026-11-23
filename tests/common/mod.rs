// Each test crate that declares `mod common;` uses a subset of these helpers.
#![allow(dead_code)]

//! Helpers shared by the backend integration tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use chaat::backend::{AuthStatus, Backend, BackendOptions, Command, Event, Waker};
use chaat::paths::AppDirs;
use chaat::secrets::MemoryStore;
use chaat::sync::{Resource, Trigger};
use wiremock::MockServer;

const CLIENT: &str = "6731de76-14a6-49ae-97bc-6eba6914391e";

pub fn id_token() -> String {
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

/// Forgets events held back by earlier waits (call before a new backend).
pub fn clear_pending() {
    PENDING.with_borrow_mut(Vec::clear);
}

/// Waits for the first event `pick` accepts, failing after `seconds`.
/// Events before it are skipped; events after it stay for the next wait.
pub fn wait_for<T>(backend: &Backend, seconds: u64, mut pick: impl FnMut(Event) -> Option<T>) -> T {
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

pub fn query_value(url: &str, key: &str) -> String {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or_default();
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .map(|value| urlencoding::decode(value).unwrap().into_owned())
        .unwrap_or_default()
}

pub struct Session {
    pub backend: Backend,
    pub dirs: AppDirs,
    pub options: BackendOptions,
    pub account: String,
    pub _dir: tempfile::TempDir,
}

/// Signs in against the mock and loads the chat list, so the cache holds
/// the chat.
pub async fn signed_in(server: &MockServer) -> Session {
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
    clear_pending();
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
pub fn cached_chats(session: Session) -> Vec<chaat::model::Chat> {
    let Session {
        backend,
        dirs,
        options,
        account,
        _dir,
    } = session;
    drop(backend);
    clear_pending();
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
