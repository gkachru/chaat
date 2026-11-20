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
        // Crashes on its first start only.
        "crash-first" => {
            let starts = log_lines_at(&log).iter().filter(|l| *l == "start").count();
            if starts == 1 {
                std::process::exit(3);
            }
            say(READY);
        }
        // Ready, then a flood of valid lines.
        "flood" => {
            say(READY);
            for _ in 0..400 {
                say(r#"{"v":1,"type":"error","message":"flood"}"#);
            }
        }
        // Ready, then a line now and then, and never a read of stdin.
        "stall" => {
            say(READY);
            for _ in 0..300 {
                std::thread::sleep(Duration::from_millis(200));
                say(r#"{"v":1,"type":"error","message":"alive"}"#);
            }
            return;
        }
        // Starts a child that outlives it (see "sleeper"), then ready.
        "grandchild" => {
            let sleeper =
                exe.with_file_name(format!("fake-sleeper{}", std::env::consts::EXE_SUFFIX));
            // Never waited on: it is meant to outlive this process.
            #[allow(clippy::zombie_processes)]
            let _child = std::process::Command::new(sleeper)
                .arg("serve")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            say(READY);
        }
        // Logs a line ten times a second, for two minutes at most.
        "sleeper" => {
            for _ in 0..1200 {
                note(&log, "beat");
                std::thread::sleep(Duration::from_millis(100));
            }
            return;
        }
        // Reports what it inherited.
        "env" => {
            let mut chaat: Vec<String> = std::env::vars_os()
                .map(|(name, _)| name.to_string_lossy().into_owned())
                .filter(|name| name.to_ascii_uppercase().starts_with("CHAAT_"))
                .collect();
            chaat.sort();
            note(&log, &format!("env-chaat:{}", chaat.join(",")));
            let keep = std::env::var("HOST_TEST_KEEP").unwrap_or_default();
            note(&log, &format!("env-keep:{keep}"));
            let cwd = std::env::current_dir().unwrap();
            note(&log, &format!("env-cwd:{}", cwd.display()));
            say(READY);
        }
        // Ready, then reads its input and never obeys a shutdown, nor leaves
        // when the input closes: only a kill ends it.
        "stubborn" => {
            say(READY);
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                note(&log, &line);
            }
            std::thread::sleep(Duration::from_secs(120));
            return;
        }
        "unsupported" => say(r#"{"v":1,"type":"ready","capabilities":["presence"]}"#),
        "display" => say(r#"{"v":1,"type":"ready","capabilities":["presence_display","x"]}"#),
        _ => say(READY),
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        note(&log, &line);
        if line.contains(r#""type":"shutdown""#) {
            std::process::exit(0);
        }
    }
}

const READY: &str = r#"{"v":1,"type":"ready","capabilities":["call_alerts"]}"#;

fn log_lines_at(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
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
    start_pinned(path, None)
}

fn start_pinned(path: PathBuf, pin: Option<String>) -> (Plugin, mpsc::Receiver<HostEvent>) {
    let (events, received) = mpsc::channel();
    let events = std::sync::Mutex::new(events);
    let plugin = Plugin::start(path, hello(), &["call_alerts"], pin, move |event| {
        let _ = events.lock().unwrap().send(event);
    });
    (plugin, received)
}

/// Every event until the host ends and drops its sink.
async fn until_the_host_ends(events: &mpsc::Receiver<HostEvent>) -> Vec<HostEvent> {
    tokio::task::block_in_place(|| {
        let deadline = std::time::Instant::now() + PATIENCE;
        let mut all = Vec::new();
        loop {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => all.push(event),
                Err(mpsc::RecvTimeoutError::Disconnected) => return all,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    assert!(std::time::Instant::now() < deadline, "the host never ended");
                }
            }
        }
    })
}

fn sha256_of(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(std::fs::read(path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The next event, waiting off the runtime so the host keeps running.
async fn next_any(events: &mpsc::Receiver<HostEvent>) -> HostEvent {
    tokio::task::block_in_place(|| events.recv_timeout(PATIENCE)).expect("an event in time")
}

/// The next event other than a fingerprint (every unpinned start reports one).
async fn next(events: &mpsc::Receiver<HostEvent>) -> HostEvent {
    loop {
        let event = next_any(events).await;
        if !matches!(event, HostEvent::Fingerprint { .. }) {
            return event;
        }
    }
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
        None,
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

#[tokio::test(flavor = "multi_thread")]
async fn an_unpinned_file_reports_its_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let path = fake(dir.path(), "echo");
    let sha = sha256_of(&path);
    let (plugin, events) = start(path);
    match next_any(&events).await {
        HostEvent::Fingerprint { sha256 } => assert_eq!(sha256, sha),
        other => panic!("expected the fingerprint first, got {other:?}"),
    }
    assert!(matches!(
        next_any(&events).await,
        HostEvent::Message(PluginMessage::Ready { .. })
    ));
    plugin.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_matching_its_pin_runs_without_a_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let path = fake(dir.path(), "echo");
    let sha = sha256_of(&path).to_uppercase();
    let (plugin, events) = start_pinned(path, Some(sha));
    assert!(matches!(
        next_any(&events).await,
        HostEvent::Message(PluginMessage::Ready { .. })
    ));
    plugin.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_file_is_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = fake(dir.path(), "echo");
    let sha = sha256_of(&path);
    let (_plugin, events) = start_pinned(path, Some("0".repeat(64)));
    let all = until_the_host_ends(&events).await;
    assert!(
        matches!(all.as_slice(), [HostEvent::FileChanged { sha256 }] if *sha256 == sha),
        "{all:?}"
    );
    assert!(
        log_lines(dir.path(), "echo").is_empty(),
        "the file was never started"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_that_stops_reading_cannot_stall_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, events) = start(fake(dir.path(), "stall"));
    assert!(matches!(
        next(&events).await,
        HostEvent::Message(PluginMessage::Ready { .. })
    ));
    // Far more than any pipe holds; the plugin reads none of it.
    let big = serde_json::json!("x".repeat(60 * 1024));
    for _ in 0..64 {
        plugin.send(HostMessage::Options {
            values: [("k".to_owned(), big.clone())].into(),
        });
    }
    // The host still reads the plugin's lines while its writes are stuck...
    let mut alive = 0;
    let error = loop {
        match next(&events).await {
            HostEvent::Message(_) => alive += 1,
            // ...until the write times out and ends the run.
            HostEvent::Stopped { error, .. } => break error,
            other => panic!("unexpected {other:?}"),
        }
    };
    assert!(alive >= 2, "read {alive} lines while writing was stuck");
    assert!(error.contains("stopped reading"), "{error}");
    let began = std::time::Instant::now();
    plugin.stop().await;
    assert!(began.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_is_cut_and_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let began = std::time::Instant::now();
    let (plugin, events) = start(fake(dir.path(), "flood"));
    let (mut messages, mut floods) = (0, 0);
    while floods == 0 {
        match next(&events).await {
            HostEvent::Message(_) => messages += 1,
            HostEvent::Flooding => floods += 1,
            _ => {}
        }
    }
    // The rest of the flood arrives and is dropped.
    tokio::time::sleep(Duration::from_secs(1)).await;
    for event in events.try_iter() {
        match event {
            HostEvent::Message(_) => messages += 1,
            HostEvent::Flooding => floods += 1,
            _ => {}
        }
    }
    plugin.stop().await;
    assert_eq!(floods, 1, "one notice per run");
    // A burst of 50, plus what the rate allows in the time it took.
    let allowed = 50 + 20 * (began.elapsed().as_secs() + 1);
    assert!(messages >= 40, "{messages} lines got through");
    assert!(
        messages as u64 <= allowed,
        "{messages} lines got through, at most {allowed} allowed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_kills_the_plugins_children() {
    let dir = tempfile::tempdir().unwrap();
    fake(dir.path(), "sleeper");
    let (plugin, events) = start(fake(dir.path(), "grandchild"));
    assert!(matches!(
        next(&events).await,
        HostEvent::Message(PluginMessage::Ready { .. })
    ));
    // The child the plugin started is running.
    tokio::task::block_in_place(|| wait_log(dir.path(), "sleeper", &["beat"]));
    plugin.stop().await;
    // It stops beating once the plugin's run is over.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let before = log_lines(dir.path(), "sleeper").len();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        log_lines(dir.path(), "sleeper").len(),
        before,
        "the plugin's child was left running"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_messages_are_not_sent_to_the_next_run() {
    let dir = tempfile::tempdir().unwrap();
    let sender = Arc::new(std::sync::OnceLock::<chaat::plugin::host::PluginSender>::new());
    let (events, received) = mpsc::channel();
    let events = std::sync::Mutex::new(events);
    let queued = sender.clone();
    let plugin = Plugin::start(
        fake(dir.path(), "crash-first"),
        hello(),
        &["call_alerts"],
        None,
        move |event| {
            // Queued while the plugin is down, before its next run begins.
            if matches!(event, HostEvent::Stopped { .. }) {
                for _ in 0..3 {
                    queued.get().unwrap().send(HostMessage::Forget {
                        account: "stale.account".into(),
                    });
                }
            }
            let _ = events.lock().unwrap().send(event);
        },
    );
    sender.set(plugin.sender()).ok().unwrap();
    // The first run crashes; the second reports ready.
    loop {
        if matches!(
            next(&received).await,
            HostEvent::Message(PluginMessage::Ready { .. })
        ) {
            break;
        }
    }
    plugin.send(HostMessage::Forget {
        account: "o3.t3".into(),
    });
    tokio::task::block_in_place(|| wait_log(dir.path(), "crash-first", &["forget", "o3.t3"]));
    plugin.stop().await;
    let lines = log_lines(dir.path(), "crash-first");
    assert!(
        lines.iter().any(|l| l.contains(r#""type":"hello""#)),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("stale.account")),
        "a message from before the restart reached the new run: {lines:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_file_is_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    let (_plugin, events) = start(dir.path().join("no-such-plugin"));
    let all = until_the_host_ends(&events).await;
    assert!(
        matches!(all.as_slice(), [HostEvent::Stopped { notify: true, .. }]),
        "{all:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relative_path_is_refused() {
    let (_plugin, events) = start(PathBuf::from("plugins").join("some-plugin"));
    let all = until_the_host_ends(&events).await;
    assert!(
        matches!(all.as_slice(), [HostEvent::Stopped { notify: true, error }] if error.contains("absolute")),
        "{all:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn chaat_variables_are_not_inherited() {
    // SAFETY: the threads of this test binary read the environment only
    // through std, which locks it, including the host starting the fakes.
    unsafe {
        std::env::set_var("CHAAT_HOST_TEST_SECRET", "s3cret");
        std::env::set_var("HOST_TEST_KEEP", "kept");
        if cfg!(windows) {
            // Names are case-insensitive there.
            std::env::set_var("Chaat_Host_Test_Mixed", "s3cret");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let (plugin, events) = start(fake(dir.path(), "env"));
    assert!(matches!(
        next(&events).await,
        HostEvent::Message(PluginMessage::Ready { .. })
    ));
    plugin.stop().await;
    let lines = log_lines(dir.path(), "env");
    let value = |key: &str| {
        lines
            .iter()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or_else(|| panic!("no {key} line in {lines:?}"))
            .to_owned()
    };
    assert_eq!(value("env-chaat:"), "", "CHAAT_ variables were inherited");
    assert_eq!(value("env-keep:"), "kept", "other variables still are");
    // It runs in its own folder.
    assert_eq!(
        std::fs::canonicalize(value("env-cwd:")).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
}

// The worker: a backend with fake plugins and a memory credential store.

fn backend(dir: &Path, store: Arc<MemoryStore>) -> Backend {
    backend_with(dir, store)
}

fn backend_with(dir: &Path, store: Arc<dyn SecretStore>) -> Backend {
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
        expected_sha256: None,
        generation: 1,
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
            ..
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
            instance: String::new(),
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
            instance: String::new(),
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

#[test]
fn stopping_a_stubborn_plugin_does_not_stall_other_commands() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), Arc::default());
    backend.send(start_command("a", fake(dir.path(), "stubborn")));
    backend.send(start_command("b", fake(dir.path(), "echo")));
    wait_ready(&backend, "a");
    wait_ready(&backend, "b");
    let asked = std::time::Instant::now();
    backend.send(Command::StopPlugin { id: "a".into() });
    backend.send(Command::PluginSend {
        id: "b".into(),
        message: HostMessage::Forget {
            account: "o3.t3".into(),
        },
    });
    wait_log(dir.path(), "echo", &[r#""type":"forget""#, "o3.t3"]);
    // The host waits up to three seconds for a plugin that ignores shutdown;
    // another plugin's message must not wait for that.
    assert!(
        asked.elapsed() < Duration::from_millis(2500),
        "b waited {:?} for a's stop",
        asked.elapsed()
    );
    wait_log(dir.path(), "stubborn", &[r#""type":"shutdown""#]);
    // Shutdown still waits for the stop under way, which ends in a kill.
    backend.shutdown();
}

#[test]
fn a_start_for_an_id_that_is_stopping_runs_after_the_old_run_ended() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), Arc::default());
    let stubborn = fake(dir.path(), "stubborn");
    backend.send(start_command("a", stubborn.clone()));
    wait_ready(&backend, "a");
    backend.send(Command::StopPlugin { id: "a".into() });
    backend.send(Command::StartPlugin {
        id: "a".into(),
        path: stubborn,
        hello: hello(),
        expected_sha256: None,
        generation: 2,
    });
    // The old run is killed after about three seconds; only then does the
    // new one start, and its events carry its generation.
    let generation = wait_for(&backend, |event| match event {
        Event::Plugin {
            id,
            generation,
            event: PluginEvent::Ready { .. },
        } if id.as_str() == "a" => Some(*generation),
        _ => None,
    });
    assert_eq!(generation, 2);
    let starts = log_lines(dir.path(), "stubborn")
        .iter()
        .filter(|line| *line == "start")
        .count();
    assert_eq!(starts, 2);
    backend.shutdown();
}

/// A credential store that holds writes for one plugin's keys until released.
struct BlockingStore {
    inner: MemoryStore,
    prefix: &'static str,
    released: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
}

impl BlockingStore {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

impl SecretStore for BlockingStore {
    fn read(&self, key: &str) -> Result<Option<String>, chaat::secrets::SecretError> {
        self.inner.read(key)
    }
    fn write(&self, key: &str, secret: &str) -> Result<(), chaat::secrets::SecretError> {
        if key.starts_with(self.prefix) {
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.changed.wait(released).unwrap();
            }
        }
        self.inner.write(key, secret)
    }
    fn delete(&self, key: &str) -> Result<(), chaat::secrets::SecretError> {
        self.inner.delete(key)
    }
}

#[test]
fn a_blocked_secret_store_for_one_plugin_does_not_hold_another() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(BlockingStore {
        inner: MemoryStore::default(),
        prefix: "plugin-option:a:",
        released: Default::default(),
        changed: Default::default(),
    });
    let mut backend = backend_with(dir.path(), store.clone());
    let save = |plugin: &str| {
        backend.send(Command::SavePluginSecret {
            plugin: plugin.into(),
            instance: String::new(),
            option: "key".into(),
            value: Some("v".into()),
        });
    };
    save("a");
    save("b");
    let first = wait_for(&backend, |event| match event {
        Event::PluginSecretSaved { plugin, ok, .. } => Some((plugin.clone(), *ok)),
        _ => None,
    });
    assert_eq!(
        first,
        ("b".to_owned(), true),
        "b is saved while a is blocked"
    );
    store.release();
    let second = wait_for(&backend, |event| match event {
        Event::PluginSecretSaved { plugin, ok, .. } => Some((plugin.clone(), *ok)),
        _ => None,
    });
    assert_eq!(second, ("a".to_owned(), true));
    backend.shutdown();
}
