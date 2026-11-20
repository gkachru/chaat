//! The plugin host against real processes. The "plugin" is this test binary
//! itself, copied under a name that picks its behaviour: when the host starts
//! it as `<copy> serve`, the test harness runs only `fake_plugin_serve`, which
//! acts as the plugin. Run normally, that test does nothing.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use chaat::backend::{Backend, BackendOptions, Command, Event, Waker};
use chaat::paths::AppDirs;
use chaat::plugin::event::PluginEvent;
use chaat::plugin::host::{HostEvent, Plugin};
use chaat::plugin::protocol::{HostMessage, PluginMessage};
use chaat::secrets::{MemoryStore, SecretStore};

/// How long a wait lasts before a test gives up. Only a hang reaches it: each
/// fake plugin is a copy of this test binary, and starting a fresh copy can
/// take many seconds when the machine is busy (the whole suite, antivirus).
const PATIENCE: Duration = Duration::from_secs(90);

/// The fake plugin. Its mode comes from the copy's name, `fake-<mode>`.
#[test]
fn fake_plugin_serve() {
    let exe = std::env::current_exe().unwrap();
    let stem = exe.file_stem().unwrap().to_string_lossy().into_owned();
    let Some(mode) = stem.strip_prefix("fake-") else {
        return; // an ordinary test run
    };
    let log = exe.with_file_name(format!("{mode}.log"));
    note(&log, "start");
    // Straight to stdout: the harness only captures `print!`.
    let say = |line: &str| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    match mode {
        "crash" => std::process::exit(3),
        "unsupported" => say(r#"{"v":1,"type":"ready","capabilities":["presence"]}"#),
        "display" => say(r#"{"v":1,"type":"ready","capabilities":["presence_display","x"]}"#),
        _ => say(r#"{"v":1,"type":"ready","capabilities":["call_alerts"]}"#),
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        note(&log, &line);
        if line.contains(r#""type":"shutdown""#) {
            std::process::exit(0);
        }
    }
}

fn note(log: &Path, line: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .unwrap();
    writeln!(file, "{line}").unwrap();
}

/// A copy of this test binary that behaves as the fake plugin `mode`.
fn fake(dir: &Path, mode: &str) -> PathBuf {
    let copy = dir.join(format!("fake-{mode}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(std::env::current_exe().unwrap(), &copy).unwrap();
    copy
}

fn hello() -> HostMessage {
    HostMessage::Hello {
        host: "chaat".into(),
        capabilities: vec!["call_alerts".into()],
        accounts: Vec::new(),
        forget: vec!["o9.t9".into()],
    }
}

fn start(path: PathBuf) -> (Plugin, mpsc::Receiver<HostEvent>) {
    let (events, received) = mpsc::channel();
    let events = std::sync::Mutex::new(events);
    let plugin = Plugin::start(path, hello(), &["call_alerts"], move |event| {
        let _ = events.lock().unwrap().send(event);
    });
    (plugin, received)
}

/// The next event, waiting off the runtime so the host keeps running.
async fn next(events: &mpsc::Receiver<HostEvent>) -> HostEvent {
    tokio::task::block_in_place(|| events.recv_timeout(PATIENCE)).expect("an event in time")
}

fn log_lines(dir: &Path, mode: &str) -> Vec<String> {
    std::fs::read_to_string(dir.join(format!("{mode}.log")))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_starts_hears_hello_and_gets_queued_messages_before_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, events) = start(fake(dir.path(), "echo"));
    assert!(matches!(
        next(&events).await,
        HostEvent::Message(PluginMessage::Ready { .. })
    ));
    // Sent and stopped at once: the forget still goes out first.
    plugin.send(HostMessage::Forget {
        account: "o1.t1".into(),
    });
    plugin.stop().await;
    let lines = log_lines(dir.path(), "echo");
    assert_eq!(lines[0], "start");
    assert!(lines[1].contains(r#""type":"hello""#) && lines[1].contains("o9.t9"));
    assert!(lines[2].contains(r#""type":"forget""#) && lines[2].contains("o1.t1"));
    assert!(lines[3].contains(r#""type":"shutdown""#));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_offering_only_unknown_capabilities_is_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let (_plugin, events) = start(fake(dir.path(), "unsupported"));
    assert!(matches!(next(&events).await, HostEvent::Unsupported));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let starts = log_lines(dir.path(), "unsupported")
        .iter()
        .filter(|line| *line == "start")
        .count();
    assert_eq!(starts, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crashing_plugin_restarts_with_one_notice_per_run_of_failures() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, events) = start(fake(dir.path(), "crash"));
    assert!(matches!(
        next(&events).await,
        HostEvent::Stopped { notify: true, .. }
    ));
    assert!(matches!(
        next(&events).await,
        HostEvent::Stopped { notify: false, .. }
    ));
    plugin.stop().await;
    assert!(
        log_lines(dir.path(), "crash").len() >= 2,
        "it was restarted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_plugin_reports_once_and_dropping_the_handle_stops_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, events) = start(dir.path().join("no-such-plugin"));
    assert!(matches!(
        next(&events).await,
        HostEvent::Stopped { notify: true, .. }
    ));
    drop(plugin);
    // With the handle gone the host ends instead of restarting forever.
    let ended = tokio::task::block_in_place(|| {
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            match events.recv_timeout(Duration::from_millis(200)) {
                Err(mpsc::RecvTimeoutError::Disconnected) => return true,
                _ if std::time::Instant::now() > deadline => return false,
                _ => {}
            }
        }
    });
    assert!(ended, "the host task ended and dropped its sink");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_offering_a_known_capability_among_others_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (events, received) = mpsc::channel();
    let events = std::sync::Mutex::new(events);
    let plugin = Plugin::start(
        fake(dir.path(), "display"),
        hello(),
        &["call_alerts", "presence_display"],
        move |event| {
            let _ = events.lock().unwrap().send(event);
        },
    );
    assert!(matches!(
        next(&received).await,
        HostEvent::Message(PluginMessage::Ready { .. })
    ));
    plugin.stop().await;
}

// The worker: a backend with fake plugins and a memory credential store.

fn backend(dir: &Path, store: Arc<MemoryStore>) -> Backend {
    Backend::spawn_with(
        AppDirs::under(dir),
        Waker::default(),
        BackendOptions {
            secrets: store,
            open_browser: false,
            ..BackendOptions::default()
        },
    )
}

fn start_command(id: &str, path: PathBuf) -> Command {
    Command::StartPlugin {
        id: id.into(),
        path,
        hello: hello(),
    }
}

thread_local! {
    /// Events polled but not yet picked by a wait: a wait takes the first
    /// match and keeps the rest for the next one.
    static PENDING: std::cell::RefCell<std::collections::VecDeque<Event>> = Default::default();
}

fn wait_for<T>(backend: &Backend, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        PENDING.with(|pending| pending.borrow_mut().extend(backend.poll()));
        // Take the first match out and leave every other event queued, in
        // order, for the next wait: plugins start in no fixed order.
        let found = PENDING.with(|pending| {
            let mut pending = pending.borrow_mut();
            let at = pending.iter().position(|event| pick(event).is_some())?;
            let event = pending.remove(at)?;
            pick(&event)
        });
        if let Some(found) = found {
            return found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for an event"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_ready(backend: &Backend, plugin: &str) {
    wait_for(backend, |event| match event {
        Event::Plugin {
            id,
            event: PluginEvent::Ready { .. },
        } if id.as_str() == plugin => Some(()),
        _ => None,
    });
}

/// Waits until the fake's log has a line holding all of `needles`.
fn wait_log(dir: &Path, mode: &str, needles: &[&str]) -> String {
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        if let Some(line) = log_lines(dir, mode)
            .into_iter()
            .find(|line| needles.iter().all(|n| line.contains(n)))
        {
            return line;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no log line with {needles:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn two_plugins_run_and_stopping_one_keeps_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), Arc::default());
    backend.send(start_command("a", fake(dir.path(), "display")));
    backend.send(start_command("b", fake(dir.path(), "echo")));
    wait_ready(&backend, "a");
    wait_ready(&backend, "b");
    backend.send(Command::StopPlugin { id: "a".into() });
    wait_log(dir.path(), "display", &[r#""type":"shutdown""#]);
    // b still takes messages.
    backend.send(Command::PluginSend {
        id: "b".into(),
        message: HostMessage::Forget {
            account: "o2.t2".into(),
        },
    });
    wait_log(dir.path(), "echo", &[r#""type":"forget""#, "o2.t2"]);
    backend.shutdown();
}

#[test]
fn restarting_one_id_replaces_only_that_host() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), Arc::default());
    // One copy: a running executable cannot be overwritten on Windows.
    let display = fake(dir.path(), "display");
    backend.send(start_command("a", display.clone()));
    backend.send(start_command("b", fake(dir.path(), "echo")));
    wait_ready(&backend, "a");
    wait_ready(&backend, "b");
    backend.send(start_command("a", display));
    wait_ready(&backend, "a");
    let starts = |mode| {
        log_lines(dir.path(), mode)
            .iter()
            .filter(|line| *line == "start")
            .count()
    };
    assert_eq!(starts("display"), 2);
    assert_eq!(starts("echo"), 1);
    backend.shutdown();
}

#[test]
fn shutdown_stops_every_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), Arc::default());
    backend.send(start_command("a", fake(dir.path(), "display")));
    backend.send(start_command("b", fake(dir.path(), "echo")));
    wait_ready(&backend, "a");
    wait_ready(&backend, "b");
    backend.shutdown();
    for mode in ["display", "echo"] {
        assert!(
            log_lines(dir.path(), mode)
                .iter()
                .any(|line| line.contains(r#""type":"shutdown""#)),
            "{mode} got a shutdown"
        );
    }
}

#[test]
fn options_include_stored_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryStore::default());
    store.write("plugin-option:b:local_key", "k").unwrap();
    let mut backend = backend(dir.path(), store);
    backend.send(start_command("b", fake(dir.path(), "echo")));
    wait_ready(&backend, "b");
    backend.send(Command::PluginOptions {
        id: "b".into(),
        values: [("ip".to_owned(), serde_json::json!("10.0.0.5"))].into(),
        secrets: vec!["local_key".into()],
    });
    let line = wait_log(dir.path(), "echo", &[r#""type":"options""#]);
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"v":1,"type":"options","values":{"ip":"10.0.0.5","local_key":"k"}})
    );
    backend.shutdown();
}

#[test]
fn a_missing_secret_is_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), Arc::default());
    backend.send(start_command("b", fake(dir.path(), "echo")));
    wait_ready(&backend, "b");
    backend.send(Command::PluginOptions {
        id: "b".into(),
        values: [("ip".to_owned(), serde_json::json!("10.0.0.5"))].into(),
        secrets: vec!["local_key".into()],
    });
    let line = wait_log(dir.path(), "echo", &[r#""type":"options""#]);
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["values"], serde_json::json!({"ip":"10.0.0.5"}));
    backend.shutdown();
}

#[test]
fn saving_a_plugin_secret_writes_and_deletes_the_store_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryStore::default());
    let mut backend = backend(dir.path(), store.clone());
    let save = |backend: &Backend, value: Option<&str>| {
        backend.send(Command::SavePluginSecret {
            plugin: "b".into(),
            option: "local_key".into(),
            value: value.map(str::to_owned),
        });
        wait_for(backend, |event| match event {
            Event::PluginSecretSaved { set, ok, .. } => Some((*set, *ok)),
            _ => None,
        })
    };
    assert_eq!(save(&backend, Some("k")), (true, true));
    assert_eq!(
        store.read("plugin-option:b:local_key").unwrap().as_deref(),
        Some("k")
    );
    assert_eq!(save(&backend, None), (false, true));
    assert_eq!(store.read("plugin-option:b:local_key").unwrap(), None);
    backend.shutdown();
}

#[test]
fn options_follow_the_secret_saved_before_them_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), Arc::default());
    backend.send(start_command("b", fake(dir.path(), "echo")));
    wait_ready(&backend, "b");
    for i in 0..30 {
        backend.send(Command::SavePluginSecret {
            plugin: "b".into(),
            option: "local_key".into(),
            value: Some(format!("k{i}")),
        });
        backend.send(Command::PluginOptions {
            id: "b".into(),
            values: Default::default(),
            secrets: vec!["local_key".into()],
        });
    }
    wait_log(dir.path(), "echo", &[r#""type":"options""#, r#""k29""#]);
    let keys: Vec<String> = log_lines(dir.path(), "echo")
        .iter()
        .filter(|line| line.contains(r#""type":"options""#))
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            value["values"]["local_key"].as_str().unwrap().to_owned()
        })
        .collect();
    let expected: Vec<String> = (0..30).map(|i| format!("k{i}")).collect();
    assert_eq!(
        keys, expected,
        "each options message holds the secret saved just before it, in order"
    );
    backend.shutdown();
}
