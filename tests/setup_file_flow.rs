//! Setup file through the service backend: export, open and apply with the
//! in-memory credential store.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chaat::backend::{Backend, BackendOptions, Command, Event, Waker};
use chaat::paths::AppDirs;
use chaat::secrets::{MemoryStore, RELAY_KEY, SecretStore};
use chaat::setup_file::SetupPayload;

const KEY: &str = "office-relay-key-0123456789";

fn wait_for<T>(backend: &Backend, mut pick: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
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

fn start(store: Arc<MemoryStore>) -> (Backend, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = store;
    let backend = Backend::spawn_with(
        AppDirs::under(dir.path()),
        Waker::default(),
        BackendOptions {
            authority: "http://127.0.0.1:9".into(),
            graph_base: "http://127.0.0.1:9/v1.0".into(),
            secrets,
            open_browser: false,
            push_renew_every: chaat::realtime::RENEW_EVERY,
            relay_connect_timeout: Duration::from_secs(1),
        },
    );
    (backend, dir)
}

fn payload() -> SetupPayload {
    SetupPayload {
        format: 1,
        created: "2026-11-21T10:00:00Z".into(),
        org_label: "Contoso".into(),
        client_id: "6731de76-14a6-49ae-97bc-6eba6914391e".into(),
        tenant: "contoso.onmicrosoft.com".into(),
        expected_tenant: None,
        relay_url: Some("https://relay.example.com".into()),
        relay_key: None,
        import_by: None,
    }
}

fn export(backend: &Backend, path: std::path::PathBuf) -> String {
    backend.send(Command::ExportSetup {
        path,
        payload: payload(),
        include_relay_key: true,
    });
    wait_for(backend, |e| match e {
        Event::SetupExported(result) => Some(result),
        _ => None,
    })
    .expect("export succeeds")
}

fn open(
    backend: &Backend,
    path: std::path::PathBuf,
    passphrase: String,
    token: u64,
) -> Result<chaat::setup_file::SetupSummary, String> {
    backend.send(Command::OpenSetupFile {
        path,
        passphrase,
        token,
    });
    wait_for(backend, |e| match e {
        Event::SetupOpened { token: t, result } if t == token => Some(result),
        _ => None,
    })
}

fn apply(backend: &Backend, token: u64) -> bool {
    backend.send(Command::ApplySetup { token });
    wait_for(backend, |e| match e {
        Event::SetupApplied { token: t, ok } if t == token => Some(ok),
        _ => None,
    })
}

#[test]
fn export_then_import_round_trip() {
    let store = Arc::new(MemoryStore::default());
    store.write(RELAY_KEY, KEY).unwrap();
    let (backend, dir) = start(store.clone());
    let file = dir.path().join("Chaat setup.chaatsetup");
    let passphrase = export(&backend, file.clone());

    // Another computer: an empty store. Mixed case and spaces are accepted.
    let other = Arc::new(MemoryStore::default());
    let (importer, _dir) = start(other.clone());
    let typed = format!("  {}  ", passphrase.replace('-', " ").to_uppercase());
    let summary = open(&importer, file, typed, 7).expect("opens");
    assert_eq!(summary.org_label, "Contoso");
    assert!(summary.relay_key_included);
    assert_eq!(other.read(RELAY_KEY).unwrap(), None);

    assert!(apply(&importer, 7));
    assert_eq!(other.read(RELAY_KEY).unwrap().as_deref(), Some(KEY));
    assert!(!apply(&importer, 7));
}

#[test]
fn wrong_passphrase_message() {
    let store = Arc::new(MemoryStore::default());
    store.write(RELAY_KEY, KEY).unwrap();
    let (backend, dir) = start(store);
    let file = dir.path().join("setup.chaatsetup");
    export(&backend, file.clone());
    let err = open(&backend, file, "not-the-passphrase".into(), 1).unwrap_err();
    assert_eq!(
        err,
        "This file can't be opened with that passphrase, or it was changed."
    );
}
