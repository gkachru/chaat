//! The plugin host against real processes. The "plugin" is this test binary
//! itself, copied under a name that picks its behaviour: when the host starts
//! it as `<copy> serve`, the test harness runs only `fake_plugin_serve`, which
//! acts as the plugin. Run normally, that test does nothing.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use chaat::plugin::host::{HostEvent, Plugin};
use chaat::plugin::protocol::{HostMessage, PluginMessage};

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
    let plugin = Plugin::start(path, hello(), "call_alerts", move |event| {
        let _ = events.lock().unwrap().send(event);
    });
    (plugin, received)
}

/// The next event, waiting off the runtime so the host keeps running.
async fn next(events: &mpsc::Receiver<HostEvent>) -> HostEvent {
    tokio::task::block_in_place(|| events.recv_timeout(Duration::from_secs(20)))
        .expect("an event in time")
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
async fn a_plugin_without_the_capability_is_stopped_and_not_restarted() {
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
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
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
