use super::*;
use crate::app::tests::*;
use crate::backend::{AuthStatus, Backend, Event, Waker};
use crate::model::Presence;
use crate::paths::AppDirs;
use crate::settings::Settings;

#[test]
fn a_ready_that_drops_accounts_clears_the_stored_restart_hello() {
    let (backend, _) = Backend::recording();
    let dir = std::env::temp_dir().join("chaat-test-unused");
    let mut app = App::with_backend(
        AppDirs::under(&dir),
        Settings::default(),
        Waker::default(),
        backend,
        false,
    );
    let (service, mut commands) = Backend::recording();
    app.service = service;
    app.id = Some("u-me.t1".into());
    app.settings.plugins.push(PluginEntry {
        id: "p".into(),
        approved_needs: vec!["accounts".into()],
        ..PluginEntry::default()
    });
    let ready = |app: &mut App, needs: Vec<&'static str>| {
        app.on_plugin_ready(
            "p",
            PluginInfo::default(),
            Vec::new(),
            needs,
            Vec::new(),
            Vec::new(),
        );
    };
    ready(&mut app, vec!["accounts"]);
    while commands.try_recv().is_ok() {}
    ready(&mut app, Vec::new());
    let sent: Vec<Command> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
    assert!(sent.iter().any(|c| matches!(
        c,
        Command::PluginHello { id, accounts: Some(a), .. } if id == "p" && a.is_empty()
    )));
}

fn option_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
    let (backend, _) = Backend::recording();
    let dir = std::env::temp_dir().join("chaat-test-unused");
    let mut app = App::with_backend(
        AppDirs::under(&dir),
        Settings::default(),
        Waker::default(),
        backend,
        false,
    );
    let (service, commands) = Backend::recording();
    app.service = service;
    app.settings.plugins.push(PluginEntry {
        id: "p".into(),
        instance: "inst".into(),
        path: PathBuf::from("p.exe"),
        enabled: true,
        sha256: Some("old".into()),
        ..PluginEntry::default()
    });
    (app, commands)
}

fn describe_options(app: &mut App) {
    let spec = |id: &str, kind| OptionSpec {
        id: id.into(),
        kind,
        label: id.into(),
        default: None,
        choices: Vec::new(),
        ..OptionSpec::default()
    };
    app.on_plugin_ready(
        "p",
        PluginInfo::default(),
        Vec::new(),
        Vec::new(),
        vec![
            spec("key", OptionKind::Secret),
            spec("name", OptionKind::Text),
        ],
        Vec::new(),
    );
}

fn option_commands(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> usize {
    std::iter::from_fn(|| commands.try_recv().ok())
        .filter(|c| matches!(c, Command::PluginOptions { .. }))
        .count()
}

fn change_and_answer(app: &mut App) {
    app.apply_plugin_action(Action::SetPluginOption {
        plugin: "p".into(),
        option: "name".into(),
        value: serde_json::json!("x"),
    });
    app.on_plugin_secret_saved("p", "inst", "key".into(), true, true);
}

#[test]
fn no_options_are_sent_before_the_new_runs_ready() {
    let (mut app, mut commands) = option_app();
    app.sync_plugins();
    describe_options(&mut app);
    while commands.try_recv().is_ok() {}
    change_and_answer(&mut app);
    assert!(option_commands(&mut commands) > 0, "after ready they go");
    // A restart: the old run's options are not sent to the new program.
    app.settings.plugins[0].path = PathBuf::from("q.exe");
    app.sync_plugins();
    change_and_answer(&mut app);
    assert_eq!(option_commands(&mut commands), 0);
    describe_options(&mut app);
    assert!(option_commands(&mut commands) > 0, "sent again on ready");
}

#[test]
fn trusting_a_changed_file_sends_options_only_after_ready() {
    let (mut app, mut commands) = option_app();
    app.sync_plugins();
    describe_options(&mut app);
    app.plugins.get_mut("p").unwrap().file_changed = Some("new".into());
    while commands.try_recv().is_ok() {}
    app.trust_plugin_file("p");
    change_and_answer(&mut app);
    assert_eq!(option_commands(&mut commands), 0);
    describe_options(&mut app);
    assert!(option_commands(&mut commands) > 0);
}

fn generation_of(command: Option<Command>) -> u64 {
    match command {
        Some(Command::StartPlugin { generation, .. }) => generation,
        other => panic!("expected a start, got {other:?}"),
    }
}

#[test]
fn events_from_an_old_generation_are_dropped() {
    let (backend, _) = Backend::recording();
    let dir = std::env::temp_dir().join("chaat-test-unused");
    let mut app = App::with_backend(
        AppDirs::under(&dir),
        Settings::default(),
        Waker::default(),
        backend,
        false,
    );
    let (service, mut commands) = Backend::recording();
    app.service = service;
    app.settings.plugins.push(PluginEntry {
        id: "p".into(),
        path: PathBuf::from("p.exe"),
        enabled: true,
        ..PluginEntry::default()
    });
    let running = |app: &mut App, generation: u64, running: bool| {
        app.on_plugin_event(
            "p".into(),
            generation,
            PluginEvent::Status {
                running,
                message: None,
            },
        );
        app.plugins["p"].running
    };
    app.sync_plugins();
    let first = generation_of(commands.try_recv().ok());
    assert!(running(&mut app, first, true), "the current run counts");
    // A new path restarts the plugin: the first run's last words are old.
    app.settings.plugins[0].path = PathBuf::from("q.exe");
    app.sync_plugins();
    let second = generation_of(commands.try_recv().ok());
    assert_ne!(first, second);
    assert!(!app.plugins["p"].running, "a restart forgets the old run");
    assert!(!running(&mut app, first, true), "the old run is ignored");
    assert!(running(&mut app, second, true), "the new run counts");
    // Turned off: nothing the stopped host still says counts, not even
    // from the latest generation.
    app.settings.plugins[0].enabled = false;
    app.sync_plugins();
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::StopPlugin { id }) if id == "p"
    ));
    assert!(!running(&mut app, second, true));
    // Turned on again: a newer generation, and only that one counts.
    app.settings.plugins[0].enabled = true;
    app.sync_plugins();
    let third = generation_of(commands.try_recv().ok());
    assert!(third > second);
    assert!(!running(&mut app, second, true));
    assert!(running(&mut app, third, true));
    // Removed: events from the plugin are dropped.
    app.remove_plugin("p");
    app.plugins.entry("p".into()).or_default().running = false;
    assert!(!running(&mut app, third, true));
}

// Options: statuses, choices, checks, actions, reset, setup.

use crate::plugin::protocol::OptionChoice;
use serde_json::json;

fn spec(id: &str, kind: OptionKind) -> OptionSpec {
    OptionSpec {
        id: id.into(),
        kind,
        label: id.into(),
        ..OptionSpec::default()
    }
}

fn choice_spec(id: &str, values: &[&str]) -> OptionSpec {
    OptionSpec {
        choices: values
            .iter()
            .map(|v| OptionChoice {
                value: (*v).into(),
                label: (*v).into(),
            })
            .collect(),
        ..spec(id, OptionKind::Choice)
    }
}

fn number_spec(id: &str, min: f64, max: f64, default: f64) -> OptionSpec {
    OptionSpec {
        min: Some(min),
        max: Some(max),
        default: Some(json!(default)),
        ..spec(id, OptionKind::Number)
    }
}

fn ready_with(app: &mut App, specs: Vec<OptionSpec>) {
    app.on_plugin_ready(
        "p",
        PluginInfo::default(),
        Vec::new(),
        Vec::new(),
        specs,
        Vec::new(),
    );
}

fn drain(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
    std::iter::from_fn(|| commands.try_recv().ok()).collect()
}

fn deliver_plugin_event(app: &mut App, event: PluginEvent) {
    let generation = app.plugin_generations.get("p").copied().unwrap_or(0);
    app.on_plugin_event("p".into(), generation, event);
}

fn status(app: &mut App, id: &str, state: StatusState, message: &str) {
    deliver_plugin_event(
        app,
        PluginEvent::OptionStatus {
            id: id.into(),
            state,
            message: Some(message.into()),
        },
    );
}

fn sent_values(
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
) -> Option<BTreeMap<String, serde_json::Value>> {
    drain(commands).into_iter().rev().find_map(|c| match c {
        Command::PluginOptions { values, .. } => Some(values),
        _ => None,
    })
}

fn action_sent(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> usize {
    drain(commands)
        .into_iter()
        .filter(|c| {
            matches!(
                c,
                Command::PluginSend {
                    message: HostMessage::Action { .. },
                    ..
                }
            )
        })
        .count()
}

#[test]
fn the_latest_status_wins_and_restart_clears_them() {
    let (mut app, _commands) = option_app();
    app.sync_plugins();
    ready_with(&mut app, vec![spec("scan", OptionKind::Action)]);
    for n in 1..=7 {
        status(&mut app, "scan", StatusState::Ok, &format!("m{n}"));
    }
    assert_eq!(
        app.plugins["p"].statuses["scan"].1.as_deref(),
        Some("m7"),
        "a burst never leaves an older status on screen"
    );
    // Only options the current `ready` describes have a status.
    status(&mut app, "unknown", StatusState::Ok, "x");
    assert!(!app.plugins["p"].statuses.contains_key("unknown"));
    // A restart forgets them.
    app.settings.plugins[0].path = PathBuf::from("q.exe");
    app.sync_plugins();
    assert!(app.plugins["p"].statuses.is_empty());
    status(&mut app, "scan", StatusState::Ok, "stale");
    assert!(
        app.plugins["p"].statuses.is_empty(),
        "nothing counts before the new run's ready"
    );
}

#[test]
fn options_update_replaces_choices_and_keeps_old_value() {
    let (mut app, _commands) = option_app();
    app.sync_plugins();
    app.settings.plugins[0]
        .options
        .insert("bulb".into(), json!("a"));
    ready_with(
        &mut app,
        vec![choice_spec("bulb", &["a"]), spec("name", OptionKind::Text)],
    );
    let update = |id: &str, values: &[&str]| PluginEvent::OptionsUpdate {
        id: id.into(),
        choices: values
            .iter()
            .map(|v| OptionChoice {
                value: (*v).into(),
                label: (*v).into(),
            })
            .collect(),
    };
    deliver_plugin_event(&mut app, update("bulb", &["b", "c"]));
    let values: Vec<&str> = app.plugins["p"].options[0]
        .choices
        .iter()
        .map(|c| c.value.as_str())
        .collect();
    assert_eq!(values, ["b", "c"]);
    assert_eq!(
        app.settings.plugins[0].options["bulb"],
        json!("a"),
        "the stored value stays"
    );
    // Not a choice option, or not described: ignored.
    deliver_plugin_event(&mut app, update("name", &["x"]));
    deliver_plugin_event(&mut app, update("nope", &["x"]));
    assert!(app.plugins["p"].options[1].choices.is_empty());
    assert_eq!(app.plugins["p"].options.len(), 2);
}

#[test]
fn invalid_values_are_not_stored_or_sent() {
    let (mut app, mut commands) = option_app();
    app.sync_plugins();
    ready_with(
        &mut app,
        vec![
            number_spec("bright", 10.0, 100.0, 50.0),
            spec("name", OptionKind::Text),
            spec("key", OptionKind::Secret),
        ],
    );
    drain(&mut commands);
    let set = |app: &mut App, option: &str, value| {
        app.apply_plugin_action(Action::SetPluginOption {
            plugin: "p".into(),
            option: option.into(),
            value,
        });
    };
    let error = |app: &App, option: &str| {
        app.plugin_option_errors
            .get(&("p".to_owned(), option.to_owned()))
            .cloned()
    };
    set(&mut app, "bright", json!(5));
    assert_eq!(
        error(&app, "bright").as_deref(),
        Some("Enter a number from 10 to 100.")
    );
    assert!(!app.settings.plugins[0].options.contains_key("bright"));
    assert!(sent_values(&mut commands).is_none(), "nothing is sent");
    // A secret never becomes a stored value, and neither does an unknown
    // option.
    set(&mut app, "key", json!("k3y"));
    set(&mut app, "nope", json!("x"));
    assert!(app.settings.plugins[0].options.is_empty());
    assert!(sent_values(&mut commands).is_none());
    // A valid value clears the error; only the normalized value is kept.
    set(&mut app, "bright", json!(60));
    assert_eq!(error(&app, "bright"), None);
    assert_eq!(app.settings.plugins[0].options["bright"], json!(60));
    assert_eq!(sent_values(&mut commands).unwrap()["bright"], json!(60));
    set(&mut app, "name", json!("  hi "));
    assert_eq!(app.settings.plugins[0].options["name"], json!("hi"));
}

#[test]
fn stored_values_that_no_longer_fit_are_dropped_on_ready() {
    let (mut app, mut commands) = option_app();
    app.sync_plugins();
    {
        let stored = &mut app.settings.plugins[0].options;
        stored.insert("bright".into(), json!(500));
        stored.insert("bulb".into(), json!(7));
        stored.insert("name".into(), json!("ok"));
        stored.insert("old".into(), json!("kept"));
    }
    ready_with(
        &mut app,
        vec![
            number_spec("bright", 10.0, 100.0, 50.0),
            choice_spec("bulb", &["a"]),
            spec("name", OptionKind::Text),
        ],
    );
    let stored = &app.settings.plugins[0].options;
    assert!(!stored.contains_key("bright"));
    assert!(!stored.contains_key("bulb"), "not a choice value");
    assert_eq!(stored["name"], json!("ok"));
    assert!(stored.contains_key("old"), "undescribed values are kept");
    let values = sent_values(&mut commands).expect("options after ready");
    assert_eq!(values["bright"], json!(50.0), "the default stands in");
    assert_eq!(values["name"], json!("ok"));
    assert!(!values.contains_key("bulb"));
}

#[test]
fn actions_go_only_to_a_ready_plugin() {
    let (mut app, mut commands) = option_app();
    let run = |app: &mut App, action: &str| {
        app.apply_plugin_action(Action::RunPluginAction {
            plugin: "p".into(),
            action: action.into(),
        });
    };
    app.sync_plugins();
    drain(&mut commands);
    run(&mut app, "scan");
    assert_eq!(action_sent(&mut commands), 0, "not before a ready");
    ready_with(
        &mut app,
        vec![
            spec("scan", OptionKind::Action),
            spec("name", OptionKind::Text),
        ],
    );
    drain(&mut commands);
    run(&mut app, "name");
    run(&mut app, "nope");
    assert_eq!(action_sent(&mut commands), 0, "only a described button");
    run(&mut app, "scan");
    let sent = drain(&mut commands);
    assert!(sent.iter().any(|c| matches!(
        c,
        Command::PluginSend {
            id,
            message: HostMessage::Action { id: action },
        } if id == "p" && action == "scan"
    )));
    assert!(app.plugins["p"].action_started.contains_key("scan"));
    // Not running: no click goes out.
    app.plugins.get_mut("p").unwrap().running = false;
    run(&mut app, "scan");
    assert_eq!(action_sent(&mut commands), 0);
    // A restart is not ready until its own ready.
    app.plugins.get_mut("p").unwrap().running = true;
    app.settings.plugins[0].path = PathBuf::from("q.exe");
    app.sync_plugins();
    app.plugins.get_mut("p").unwrap().running = true;
    drain(&mut commands);
    run(&mut app, "scan");
    assert_eq!(action_sent(&mut commands), 0);
}

#[test]
fn an_action_is_working_until_its_status_or_30s() {
    let (mut app, _commands) = option_app();
    app.sync_plugins();
    ready_with(&mut app, vec![spec("scan", OptionKind::Action)]);
    let now = Instant::now();
    assert!(!app.plugins["p"].action_working("scan", now));
    let run = |app: &mut App| {
        app.apply_plugin_action(Action::RunPluginAction {
            plugin: "p".into(),
            action: "scan".into(),
        });
    };
    run(&mut app);
    assert!(app.plugins["p"].action_working("scan", Instant::now()));
    assert!(
        !app.plugins["p"].action_working("scan", Instant::now() + Duration::from_secs(31)),
        "30 seconds without an answer ends the wait"
    );
    status(&mut app, "scan", StatusState::Working, "Looking");
    assert!(app.plugins["p"].action_working("scan", Instant::now()));
    status(&mut app, "scan", StatusState::Ok, "Found 2 bulbs");
    assert!(!app.plugins["p"].action_working("scan", Instant::now()));
    assert!(app.plugins["p"].action_started.is_empty());
    // The same for an error, and a click clears the old answer.
    run(&mut app);
    assert!(!app.plugins["p"].statuses.contains_key("scan"));
    status(&mut app, "scan", StatusState::Error, "No bulbs");
    assert!(!app.plugins["p"].action_working("scan", Instant::now()));
}

#[test]
fn reset_section_restores_defaults_and_keeps_secrets() {
    let (mut app, mut commands) = option_app();
    app.sync_plugins();
    let in_section = |section: &str, spec: OptionSpec| OptionSpec {
        section: Some(section.into()),
        ..spec
    };
    ready_with(
        &mut app,
        vec![
            in_section("Colors", number_spec("bright", 10.0, 100.0, 50.0)),
            in_section("Colors", spec("note", OptionKind::Text)),
            in_section("Colors", spec("key", OptionKind::Secret)),
            in_section("Bulb", number_spec("other", 1.0, 100.0, 10.0)),
        ],
    );
    {
        let entry = &mut app.settings.plugins[0];
        entry.options.insert("bright".into(), json!(70));
        entry.options.insert("note".into(), json!("x"));
        entry.options.insert("other".into(), json!(20));
        entry.secrets = vec!["key".into()];
    }
    app.plugin_option_errors
        .insert(("p".into(), "bright".into()), "Enter a number.".into());
    drain(&mut commands);
    app.apply_plugin_action(Action::ResetPluginSection {
        plugin: "p".into(),
        section: Some("Colors".into()),
    });
    let stored = &app.settings.plugins[0].options;
    assert_eq!(stored["bright"], json!(50.0));
    assert!(!stored.contains_key("note"), "no default: cleared");
    assert_eq!(stored["other"], json!(20), "other sections stay");
    assert_eq!(app.settings.plugins[0].secrets, ["key"]);
    assert!(app.plugin_option_errors.is_empty());
    let sent = drain(&mut commands);
    assert!(sent.iter().any(|c| matches!(
        c,
        Command::PluginOptions { values, secrets, .. }
            if values["bright"] == json!(50.0) && secrets == &["key".to_owned()]
    )));
    assert!(
        !sent
            .iter()
            .any(|c| matches!(c, Command::SavePluginSecret { .. })),
        "secrets are untouched"
    );
}

#[test]
fn a_new_plugin_opens_its_add_flow_and_stays_pending() {
    let (mut app, mut commands) = option_app();
    let required = |id: &str, kind| OptionSpec {
        required: true,
        ..spec(id, kind)
    };
    app.sync_plugins();
    ready_with(&mut app, vec![required("key", OptionKind::Secret)]);
    assert_eq!(
        app.plugins_view,
        PluginsView::List,
        "an old plugin: no flow"
    );
    assert_eq!(app.plugin_missing_required("p"), ["key"]);
    app.settings.plugins[0].secrets = vec!["key".into()];
    assert!(app.plugin_missing_required("p").is_empty());
    app.settings.plugins[0].secrets.clear();
    drain(&mut commands);
    app.add_plugin(PathBuf::from("added-plugin.exe"));
    let id = app.settings.plugins.last().unwrap().id.clone();
    assert!(app.settings.plugins.last().unwrap().setup_pending);
    assert_eq!(app.plugins_view, PluginsView::Adding(id.clone()));
    assert!(
        drain(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::StartPlugin { id: started, .. } if *started == id)),
        "it starts so that it can send ready"
    );
    app.remove_plugin(&id);
    assert_eq!(app.plugins_view, PluginsView::List);
}

#[test]
fn setup_pending_is_not_started_at_launch() {
    let (mut app, mut commands) = option_app();
    app.settings.plugins.push(PluginEntry {
        id: "new".into(),
        path: PathBuf::from("new.exe"),
        enabled: true,
        setup_pending: true,
        ..PluginEntry::default()
    });
    app.sync_plugins();
    let started: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter_map(|c| match c {
            Command::StartPlugin { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(started, ["p"], "only the finished plugin runs");
    assert_eq!(
        app.plugin_summary("new"),
        PluginSummary {
            level: SummaryLevel::Attention,
            text: "Finish setup".into()
        }
    );
    // Opening its setup page starts it; leaving Settings stops it again.
    app.apply_plugin_action(Action::OpenPlugin("new".into()));
    assert_eq!(app.plugins_view, PluginsView::Adding("new".into()));
    assert_eq!(app.settings_page, Some(SettingsPage::Plugins));
    assert!(
        drain(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::StartPlugin { id, .. } if id == "new"))
    );
    app.settings_page = None;
    app.plugins_view_follows_settings();
    assert_eq!(app.plugins_view, PluginsView::List);
    assert!(
        drain(&mut commands)
            .iter()
            .any(|c| matches!(c, Command::StopPlugin { id } if id == "new"))
    );
    assert!(app.settings.plugins[1].setup_pending, "still pending");
}

#[test]
fn old_settings_load_setup_pending_false() {
    let entry: PluginEntry =
        serde_json::from_str(r#"{"id":"p","path":"p.exe","enabled":true}"#).unwrap();
    assert!(!entry.setup_pending);
}

fn pending_app() -> App {
    let (mut app, _commands) = option_app();
    app.settings.plugins[0].setup_pending = true;
    app.settings.plugins[0].needs = vec!["accounts".into(), "presence".into()];
    app.sync_plugins_for_test();
    app
}

impl App {
    /// Starts "p" as the add flow does, then answers with a `ready` that
    /// asks for accounts and presence.
    fn sync_plugins_for_test(&mut self) {
        self.plugins_view = PluginsView::Adding("p".into());
        self.sync_plugins();
        self.on_plugin_ready(
            "p",
            PluginInfo::default(),
            Vec::new(),
            vec!["accounts", "presence"],
            Vec::new(),
            Vec::new(),
        );
    }
}

#[test]
fn finish_setup_approves_ticked_refuses_unticked() {
    let mut app = pending_app();
    assert_eq!(app.plugins["p"].pending_needs, ["accounts", "presence"]);
    app.apply_plugin_action(Action::FinishPluginSetup {
        id: "p".into(),
        allow: vec!["presence".into()],
    });
    let entry = &app.settings.plugins[0];
    assert!(!entry.setup_pending);
    assert_eq!(entry.approved_needs, ["presence"]);
    assert_eq!(entry.denied_needs, ["accounts"]);
    assert!(app.plugins["p"].pending_needs.is_empty());
    assert_eq!(app.plugins_view, PluginsView::Plugin("p".into()));
    assert!(!App::plugin_need_effective(entry, "accounts"));
    assert!(App::plugin_need_effective(entry, "presence"));
}

#[test]
fn finish_setup_with_nothing_ticked_sends_no_accounts() {
    let mut app = pending_app();
    let (service, mut commands) = Backend::recording();
    app.service = service;
    app.apply_plugin_action(Action::FinishPluginSetup {
        id: "p".into(),
        allow: Vec::new(),
    });
    assert_eq!(app.settings.plugins[0].denied_needs.len(), 2);
    assert!(app.settings.plugins[0].approved_needs.is_empty());
    assert!(!drain(&mut commands).iter().any(|c| matches!(
        c,
        Command::PluginSend {
            message: HostMessage::Accounts { .. } | HostMessage::Presence { .. },
            ..
        }
    )));
}

#[test]
fn finish_setup_waits_for_required_options() {
    let mut app = pending_app();
    app.on_plugin_ready(
        "p",
        PluginInfo::default(),
        Vec::new(),
        Vec::new(),
        vec![OptionSpec {
            required: true,
            ..spec("name", OptionKind::Text)
        }],
        Vec::new(),
    );
    app.apply_plugin_action(Action::FinishPluginSetup {
        id: "p".into(),
        allow: Vec::new(),
    });
    assert!(
        app.settings.plugins[0].setup_pending,
        "a required option is empty"
    );
    assert_eq!(app.plugins_view, PluginsView::Adding("p".into()));
}

#[test]
fn cancel_setup_removes() {
    let mut app = pending_app();
    app.apply_plugin_action(Action::CancelPluginSetup("p".into()));
    assert!(app.settings.plugins.is_empty());
    assert!(!app.plugins.contains_key("p"));
    assert_eq!(app.plugins_view, PluginsView::List);
    // A finished plugin is not removed by a stray cancel.
    let (mut app, _commands) = option_app();
    app.apply_plugin_action(Action::CancelPluginSetup("p".into()));
    assert_eq!(app.settings.plugins.len(), 1);
}

#[test]
fn leaving_settings_resets_the_view_and_reordering() {
    let (mut app, _commands) = option_app();
    app.settings.plugins.push(PluginEntry {
        id: "q".into(),
        ..PluginEntry::default()
    });
    app.apply_plugin_action(Action::OpenPlugin("p".into()));
    assert_eq!(app.plugins_view, PluginsView::Plugin("p".into()));
    app.apply_plugin_action(Action::BackToPlugins);
    app.apply_plugin_action(Action::StartReorder);
    assert!(app.reordering);
    app.apply_plugin_action(Action::EndReorder);
    assert!(!app.reordering);
    app.apply_plugin_action(Action::StartReorder);
    app.settings_page = None;
    app.plugins_view_follows_settings();
    assert!(!app.reordering);
    // Reordering needs two plugins.
    app.settings.plugins.pop();
    app.apply_plugin_action(Action::StartReorder);
    assert!(!app.reordering);
}

/// Two plugins that offer push and call alerts, both running and
/// allowed, in an app with one account whose backend is returned.
fn two_providers() -> (
    App,
    tokio::sync::mpsc::UnboundedReceiver<Command>,
    tokio::sync::mpsc::UnboundedReceiver<Command>,
) {
    let (mut app, mut service) = option_app();
    app.settings.plugins.clear();
    let (account_backend, mut account) = Backend::recording();
    app.accounts.push(crate::app::Account::new(account_backend));
    for id in ["a", "b"] {
        app.settings.plugins.push(PluginEntry {
            id: id.into(),
            path: PathBuf::from(format!("{id}.exe")),
            enabled: true,
            needs: vec!["push".into()],
            approved_needs: vec!["push".into()],
            ..PluginEntry::default()
        });
        app.plugins.insert(
            id.into(),
            PluginState {
                running: true,
                offers: vec![registry::CALL_ALERTS, registry::PUSH],
                needs: vec!["push"],
                ready_seen: true,
                ..PluginState::default()
            },
        );
        app.plugins_started
            .insert(id.into(), PathBuf::from(format!("{id}.exe")));
        let url = format!("https://{id}.trycloudflare.com");
        app.on_plugin_event(
            id.into(),
            0,
            PluginEvent::Push(crate::plugin::event::PushEvent::Ready { url }),
        );
    }
    drain(&mut service);
    drain(&mut account);
    (app, service, account)
}

#[test]
fn move_plugin_switches_owners() {
    let (mut app, mut service, mut account) = two_providers();
    assert_eq!(app.push_owner().as_deref(), Some("a"));
    assert_eq!(app.call_alerts_owner().as_deref(), Some("a"));
    assert_eq!(
        app.plugin_provides("a"),
        [registry::PUSH, registry::CALL_ALERTS]
    );
    assert!(app.plugin_provides("b").is_empty());
    app.plugins
        .get_mut("a")
        .unwrap()
        .sign_in
        .push(PluginSignIn {
            plugin: "a".into(),
            username: None,
            account: None,
            verification_uri: "https://x".into(),
            user_code: "C".into(),
        });
    // Moving to the same place changes nothing.
    app.apply_plugin_action(Action::MovePlugin {
        id: "a".into(),
        to: 0,
    });
    assert!(drain(&mut service).is_empty() && drain(&mut account).is_empty());
    assert_eq!(app.plugins["a"].sign_in.len(), 1);
    app.apply_plugin_action(Action::MovePlugin {
        id: "b".into(),
        to: 0,
    });
    let ids: Vec<&str> = app.settings.plugins.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["b", "a"]);
    assert_eq!(app.push_owner().as_deref(), Some("b"));
    assert_eq!(app.call_alerts_owner().as_deref(), Some("b"));
    assert_eq!(
        app.plugin_provides("b"),
        [registry::PUSH, registry::CALL_ALERTS]
    );
    assert!(app.plugin_provides("a").is_empty());
    assert!(
        app.plugins["a"].sign_in.is_empty(),
        "the old owner's cards go"
    );
    let sent = drain(&mut service);
    assert!(sent.iter().any(|c| matches!(
        c,
        Command::PluginSend { id, message: HostMessage::PushChannels { channels } }
            if id == "a" && channels.is_empty()
    )));
    assert!(sent.iter().any(|c| matches!(
        c,
        Command::PluginSend { id, message: HostMessage::PushChannels { channels } }
            if id == "b" && channels.len() == app.accounts.len()
    )));
    assert!(drain(&mut account).iter().any(|c| matches!(
        c,
        Command::PushPlugin(Some(route)) if format!("{route:?}").contains("b.trycloudflare.com")
    )));
    // An index past the end moves it last.
    app.apply_plugin_action(Action::MovePlugin {
        id: "b".into(),
        to: 99,
    });
    assert_eq!(app.settings.plugins[1].id, "b");
    assert_eq!(app.push_owner().as_deref(), Some("a"));
}

#[test]
fn plugin_provides_leaves_push_out_while_a_relay_is_set() {
    let (mut app, ..) = two_providers();
    app.settings.relay_url = Some("https://relay.example".into());
    assert_eq!(app.plugin_provides("a"), [registry::CALL_ALERTS]);
}

#[test]
fn summary_ignores_push_while_a_relay_is_set() {
    let (mut app, ..) = two_providers();
    app.plugins.get_mut("a").unwrap().offers = vec![registry::PUSH];
    app.plugins.get_mut("b").unwrap().offers = vec![registry::PUSH];
    assert_eq!(
        app.plugin_summary("a").text,
        "Tunnel at a.trycloudflare.com"
    );
    assert_eq!(
        app.plugin_summary("b").text,
        "Not used: another plugin provides push"
    );
    app.settings.relay_url = Some("https://relay.example".into());
    assert_eq!(app.plugin_summary("a").text, "Running");
    assert_eq!(app.plugin_summary("b").text, "Running");
}

#[test]
fn move_plugin_ignores_unknown_ids_and_same_index() {
    let (mut app, mut service, _account) = two_providers();
    app.apply_plugin_action(Action::MovePlugin {
        id: "nope".into(),
        to: 0,
    });
    app.apply_plugin_action(Action::MovePlugin {
        id: "b".into(),
        to: 1,
    });
    app.apply_plugin_action(Action::MovePlugin {
        id: "b".into(),
        to: 99,
    });
    let ids: Vec<&str> = app.settings.plugins.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
    assert!(drain(&mut service).is_empty());
}

#[test]
fn add_start_times_out_after_the_limit_without_ready() {
    let (mut app, ..) = option_app();
    assert!(!app.plugin_start_timed_out("p"), "never started");
    app.plugin_started_at.insert("p".into(), Instant::now());
    assert!(!app.plugin_start_timed_out("p"));
    if let Some(past) = Instant::now().checked_sub(ADD_START_LIMIT + Duration::from_secs(1)) {
        app.plugin_started_at.insert("p".into(), past);
        app.plugins.entry("p".into()).or_default().ready_seen = false;
        assert!(app.plugin_start_timed_out("p"));
        app.plugins.get_mut("p").unwrap().ready_seen = true;
        assert!(!app.plugin_start_timed_out("p"), "ready ends the wait");
    }
}

#[test]
fn turn_off_question_texts() {
    let (mut app, ..) = two_providers();
    app.plugins.get_mut("a").unwrap().info.name = Some("Alpha".into());
    app.plugins.get_mut("b").unwrap().info.name = Some("Beta".into());
    // Provides nothing: no question.
    assert_eq!(app.turn_off_question("b"), None);
    assert_eq!(app.turn_off_question("nope"), None);
    // Both, another takes over.
    assert_eq!(
        app.turn_off_question("a").as_deref(),
        Some(
            "Turn off “Alpha”? “Beta” provides push updates instead. “Beta” provides call alerts instead."
        )
    );
    // Push only, nothing else offers it; call alerts none either.
    app.plugins.get_mut("b").unwrap().offers.clear();
    assert_eq!(
        app.turn_off_question("a").as_deref(),
        Some(
            "Turn off “Alpha”? Push updates stop until you turn it back on or set a relay address. Chaat stops showing incoming calls from this plugin."
        )
    );
    // Push only.
    app.plugins.get_mut("a").unwrap().offers = vec![registry::PUSH];
    assert_eq!(
        app.turn_off_question("a").as_deref(),
        Some(
            "Turn off “Alpha”? Push updates stop until you turn it back on or set a relay address."
        )
    );
    // Calls only.
    app.plugins.get_mut("a").unwrap().offers = vec![registry::CALL_ALERTS];
    assert_eq!(
        app.turn_off_question("a").as_deref(),
        Some("Turn off “Alpha”? Chaat stops showing incoming calls from this plugin.")
    );
    // Calls only, another takes over.
    app.plugins.get_mut("b").unwrap().offers = vec![registry::CALL_ALERTS];
    assert_eq!(
        app.turn_off_question("a").as_deref(),
        Some("Turn off “Alpha”? “Beta” provides call alerts instead.")
    );
    // A relay address set: push is left out, and with nothing else
    // provided no question is asked.
    app.plugins.get_mut("a").unwrap().offers = vec![registry::PUSH];
    app.settings.relay_url = Some("https://relay.example".into());
    assert_eq!(app.turn_off_question("a"), None);
}

#[test]
fn turn_off_question_quotes_names_a_plugin_chose() {
    let (mut app, ..) = two_providers();
    app.plugins.get_mut("a").unwrap().info.name = Some("Al\u{201C}pha\u{201D}".into());
    app.plugins.get_mut("b").unwrap().info.name = Some("Be, ta".into());
    assert_eq!(app.quoted_plugin_name("a"), "\u{201C}Al'pha'\u{201D}");
    assert_eq!(
        app.turn_off_question("a").as_deref(),
        Some(
            "Turn off \u{201C}Al'pha'\u{201D}? \u{201C}Be, ta\u{201D} provides push updates instead. \u{201C}Be, ta\u{201D} provides call alerts instead."
        )
    );
    assert_eq!(
        app.turn_off_consequences("a").as_deref(),
        Some(
            "\u{201C}Be, ta\u{201D} provides push updates instead. \u{201C}Be, ta\u{201D} provides call alerts instead."
        )
    );
}

#[test]
fn retry_stops_the_old_run_and_ignores_finished_plugins() {
    let (mut app, mut commands) = option_app();
    app.settings.plugins[0].setup_pending = true;
    app.plugins_view = PluginsView::Adding("p".into());
    app.sync_plugins();
    let first = app.plugin_generations["p"];
    let before = app.plugin_started_at["p"];
    while commands.try_recv().is_ok() {}
    app.apply_plugin_action(Action::RetryPluginStart("p".into()));
    let sent: Vec<Command> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
    let stop = sent
        .iter()
        .position(|c| matches!(c, Command::StopPlugin { id } if id == "p"));
    let start = sent
        .iter()
        .position(|c| matches!(c, Command::StartPlugin { id, .. } if id == "p"));
    assert!(
        stop.is_some() && start.is_some() && stop < start,
        "stop, then start"
    );
    let second = app.plugin_generations["p"];
    assert!(second > first);
    assert!(
        !app.plugin_event_counts("p", first),
        "a late event from the old run is dropped"
    );
    assert!(app.plugin_event_counts("p", second));
    assert!(app.plugin_started_at["p"] >= before, "the wait restarts");
    // Not in an add flow, or not in the list: nothing happens.
    app.settings.plugins[0].setup_pending = false;
    app.apply_plugin_action(Action::RetryPluginStart("p".into()));
    app.apply_plugin_action(Action::RetryPluginStart("nope".into()));
    assert!(commands.try_recv().is_err());
    assert_eq!(app.plugin_generations["p"], second);
}

#[test]
fn plugin_start_remaining_counts_down_and_ends_with_ready() {
    let (mut app, ..) = option_app();
    assert_eq!(app.plugin_start_remaining("p"), None, "never started");
    app.plugin_started_at.insert("p".into(), Instant::now());
    let left = app.plugin_start_remaining("p").unwrap();
    assert!(left > Duration::ZERO && left <= ADD_START_LIMIT);
    if let Some(past) = Instant::now().checked_sub(ADD_START_LIMIT + Duration::from_secs(1)) {
        app.plugin_started_at.insert("p".into(), past);
        assert_eq!(app.plugin_start_remaining("p"), Some(Duration::ZERO));
        app.plugins.entry("p".into()).or_default().ready_seen = true;
        assert_eq!(app.plugin_start_remaining("p"), None);
    }
}

fn summary_of(app: &App, id: &str) -> (SummaryLevel, String) {
    let summary = app.plugin_summary(id);
    (summary.level, summary.text)
}

#[test]
fn summary_order_follows_the_table() {
    use SummaryLevel::*;
    let (mut app, ..) = two_providers();
    // 10: running, offers nothing exclusive.
    app.plugins.get_mut("b").unwrap().offers.clear();
    assert_eq!(summary_of(&app, "b"), (Ok, "Running".into()));
    // 9: another plugin owns what it offers.
    app.plugins.get_mut("b").unwrap().offers = vec![registry::CALL_ALERTS];
    assert_eq!(
        summary_of(&app, "b"),
        (Off, "Not used: another plugin provides calls".into())
    );
    app.plugins.get_mut("b").unwrap().offers = vec![registry::PUSH];
    assert_eq!(
        summary_of(&app, "b"),
        (Off, "Not used: another plugin provides push".into())
    );
    // 8: push owner, tunnel up, then down, then waiting.
    app.plugins.get_mut("a").unwrap().offers = vec![registry::PUSH];
    assert_eq!(
        summary_of(&app, "a"),
        (Ok, "Tunnel at a.trycloudflare.com".into())
    );
    app.on_plugin_event(
        "a".into(),
        0,
        PluginEvent::Push(crate::plugin::event::PushEvent::Down { reason: None }),
    );
    assert_eq!(summary_of(&app, "a"), (Ok, "Tunnel down".into()));
    app.push_plugin_stopped("a");
    assert_eq!(summary_of(&app, "a"), (Ok, "Waiting for its tunnel".into()));
    // 7: call alerts owner.
    app.plugins.get_mut("a").unwrap().offers = vec![registry::CALL_ALERTS];
    app.plugins.get_mut("a").unwrap().listening = vec!["u1.t".into()];
    assert_eq!(summary_of(&app, "a"), (Ok, "Listening on 1 account".into()));
    app.plugins.get_mut("a").unwrap().listening = vec!["u1.t".into(), "u2.t".into()];
    assert_eq!(
        summary_of(&app, "a"),
        (Ok, "Listening on 2 accounts".into())
    );
    let state = app.plugins.get_mut("a").unwrap();
    state.listening.clear();
    state.sign_in.push(PluginSignIn {
        plugin: "a".into(),
        username: None,
        account: None,
        verification_uri: "https://x".into(),
        user_code: "C".into(),
    });
    assert_eq!(
        summary_of(&app, "a"),
        (Ok, "Waiting for sign-in on 1".into())
    );
    // 6: enabled, not running yet.
    app.plugins.get_mut("a").unwrap().running = false;
    assert_eq!(summary_of(&app, "a"), (Off, "Starting\u{2026}".into()));
    // 4: stopped with a reason.
    app.plugins.get_mut("a").unwrap().message = Some("The file could not be run".into());
    assert_eq!(
        summary_of(&app, "a"),
        (Error, "Stopped: The file could not be run".into())
    );
    // 5: turned off beats starting and stopped, not the rules above.
    app.settings.plugins[0].enabled = false;
    assert_eq!(summary_of(&app, "a"), (Off, "Off".into()));
    app.settings.plugins[0].enabled = true;
    // 3: a required option unset beats stopped.
    app.plugins.get_mut("a").unwrap().options = vec![OptionSpec {
        required: true,
        ..spec("name", OptionKind::Text)
    }];
    assert_eq!(summary_of(&app, "a"), (Attention, "Needs setup".into()));
    // 2: a pending need beats an unset option.
    app.plugins.get_mut("a").unwrap().pending_needs = vec!["accounts", "push"];
    assert_eq!(
        summary_of(&app, "a"),
        (Attention, "Asks for your accounts".into())
    );
    app.plugins.get_mut("a").unwrap().pending_needs = vec!["presence"];
    assert_eq!(
        summary_of(&app, "a"),
        (Attention, "Asks for your presence".into())
    );
    app.plugins.get_mut("a").unwrap().pending_needs = vec!["push"];
    assert_eq!(
        summary_of(&app, "a"),
        (Attention, "Asks for your update notifications".into())
    );
    // 1: a changed file beats everything.
    app.plugins.get_mut("a").unwrap().file_changed = Some("h".into());
    assert_eq!(
        summary_of(&app, "a"),
        (Attention, "The file changed. Review it".into())
    );
    assert!(app.plugin_summary("a").needs_you());
    assert!(!app.plugin_summary("b").needs_you());
    // A plugin in the add flow that stopped before ready.
    app.settings.plugins[1].setup_pending = true;
    app.plugins.get_mut("b").unwrap().ready_seen = false;
    assert_eq!(summary_of(&app, "b"), (Attention, "Finish setup".into()));
    app.plugins.get_mut("b").unwrap().message = Some("x".into());
    assert_eq!(
        summary_of(&app, "b"),
        (Error, "It stopped before saying what it does".into())
    );
}

#[test]
fn a_stored_choice_survives_a_restart_until_options_update() {
    let (mut app, mut commands) = option_app();
    app.sync_plugins();
    app.settings.plugins[0]
        .options
        .insert("bulb".into(), json!("old-bulb"));
    // The new run does not list it (yet): it is kept and still sent.
    ready_with(&mut app, vec![choice_spec("bulb", &["a"])]);
    assert_eq!(app.settings.plugins[0].options["bulb"], json!("old-bulb"));
    assert_eq!(
        sent_values(&mut commands).unwrap()["bulb"],
        json!("old-bulb")
    );
    let update = |values: &[&str]| PluginEvent::OptionsUpdate {
        id: "bulb".into(),
        choices: values
            .iter()
            .map(|v| OptionChoice {
                value: (*v).into(),
                label: (*v).into(),
            })
            .collect(),
    };
    deliver_plugin_event(&mut app, update(&["b"]));
    assert_eq!(app.settings.plugins[0].options["bulb"], json!("old-bulb"));
    // Choosing it by hand still needs it to be offered.
    app.apply_plugin_action(Action::SetPluginOption {
        plugin: "p".into(),
        option: "bulb".into(),
        value: json!("zzz"),
    });
    assert_eq!(app.settings.plugins[0].options["bulb"], json!("old-bulb"));
    // A malformed stored value still goes.
    app.settings.plugins[0]
        .options
        .insert("bulb".into(), json!("x".repeat(65)));
    ready_with(&mut app, vec![choice_spec("bulb", &["a"])]);
    assert!(!app.settings.plugins[0].options.contains_key("bulb"));
}

#[test]
fn secrets_are_checked_and_trimmed_before_saving() {
    let (mut app, mut commands) = option_app();
    ready_with(
        &mut app,
        vec![
            spec("key", OptionKind::Secret),
            spec("note", OptionKind::Text),
        ],
    );
    drain(&mut commands);
    let set = |app: &mut App, option: &str, value: &str| {
        app.apply_plugin_action(Action::SetPluginSecret {
            plugin: "p".into(),
            option: option.into(),
            value: Some(value.to_owned()),
        });
    };
    set(&mut app, "key", "  k3y \n");
    let saved: Vec<_> = drain(&mut commands)
        .into_iter()
        .filter_map(|c| match c {
            Command::SavePluginSecret { value, .. } => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(saved, [Some("k3y".to_owned())]);
    let key = ("p".to_owned(), "key".to_owned());
    for bad in ["   ", "a\nb", "a\u{202E}b", &"x".repeat(1001)] {
        set(&mut app, "key", bad);
        assert!(drain(&mut commands).is_empty(), "{bad:?} not saved");
        assert!(app.plugin_option_errors.contains_key(&key));
    }
    set(&mut app, "note", "v");
    assert!(drain(&mut commands).is_empty(), "not a secret");
    set(&mut app, "missing", "v");
    assert!(drain(&mut commands).is_empty());
}

fn busy() -> Presence {
    Presence {
        availability: crate::model::Availability::Busy,
        ..Default::default()
    }
}

fn app_with_service() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
    let (mut app, _) = app();
    let (service, commands) = Backend::recording();
    app.service = service;
    app.id = Some("u-me.t1".into());
    (app, commands)
}

#[test]
fn the_plugin_sign_in_panel_shows_until_signed_in_or_stopped() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    call_alerts_plugin(&mut app, "ca");
    app.handle(plugin_event(&app, "ca", sign_in_event(None, "AB12")), &ctx);
    assert_eq!(app.plugins["ca"].sign_in[0].user_code, "AB12");
    app.handle(plugin_event(&app, "ca", signed_in_event(None)), &ctx);
    assert!(app.plugins["ca"].sign_in.is_empty());
    app.handle(plugin_event(&app, "ca", sign_in_event(None, "AB12")), &ctx);
    app.handle(
        plugin_event(
            &app,
            "ca",
            crate::plugin::event::PluginEvent::Status {
                running: false,
                message: None,
            },
        ),
        &ctx,
    );
    assert!(app.plugins["ca"].sign_in.is_empty());
    app.handle(plugin_event(&app, "ca", sign_in_event(None, "AB12")), &ctx);
    app.apply(
        Action::HidePluginSignIn {
            plugin: "ca".into(),
            account: None,
        },
        &ctx,
    );
    assert!(app.plugins["ca"].sign_in.is_empty());
}

#[test]
fn plugins_turned_on_start_once_the_service_is_attached() {
    // At launch App::new syncs before main attaches the service backend.
    let (mut app, _) = app();
    assert!(app.service.is_detached());
    app.settings.plugins.push(crate::settings::PluginEntry {
        id: "pr".into(),
        path: PathBuf::from("plugin.exe"),
        enabled: true,
        ..Default::default()
    });
    app.sync_plugins();
    let (service, mut commands) = Backend::recording();
    app.attach_service(service);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::StartPlugin { id, .. }) if id == "pr"
    ));
}

#[test]
fn enabling_and_disabling_a_plugin_starts_and_stops_it() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    app.settings.plugins.push(crate::settings::PluginEntry {
        id: "ca".into(),
        path: PathBuf::from("plugin.exe"),
        enabled: false,
        needs: vec!["accounts".into()],
        approved_needs: vec!["accounts".into()],
        ..Default::default()
    });
    app.sync_plugins();
    assert!(commands.try_recv().is_err(), "off: nothing starts");
    app.apply(Action::SetPluginEnabled("ca".into(), true), &ctx);
    let me = crate::calling::PluginAccount {
        id: "u-me.t1".into(),
        username: "me@contoso.com".into(),
    };
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::StartPlugin { id, path, hello: crate::plugin::protocol::HostMessage::Hello { accounts, .. }, .. })
            if id == "ca" && path == std::path::Path::new("plugin.exe") && accounts == vec![me.clone()]
    ));
    // Syncing again with nothing changed starts nothing.
    app.sync_plugins();
    assert!(commands.try_recv().is_err());
    // A new path restarts that plugin only.
    app.settings.plugins[0].path = PathBuf::from("other.exe");
    app.sync_plugins();
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::StartPlugin { path, .. }) if path == std::path::Path::new("other.exe")
    ));
    app.apply(Action::SetPluginEnabled("ca".into(), false), &ctx);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::StopPlugin { id }) if id == "ca"
    ));
}

#[test]
fn only_the_first_sign_in_code_raises_the_window() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    call_alerts_plugin(&mut app, "ca");
    app.wants_show = false;
    app.handle(plugin_event(&app, "ca", sign_in_event(None, "FIRST")), &ctx);
    assert!(app.wants_show);
    app.wants_show = false;
    app.apply(
        Action::HidePluginSignIn {
            plugin: "ca".into(),
            account: None,
        },
        &ctx,
    );
    app.handle(
        plugin_event(&app, "ca", sign_in_event(None, "SECOND")),
        &ctx,
    );
    assert!(!app.wants_show, "a later code doesn't pull Chaat forward");
    assert_eq!(app.plugins["ca"].sign_in[0].user_code, "SECOND");
    // After signing in, a future expiry may raise it again.
    app.handle(plugin_event(&app, "ca", signed_in_event(None)), &ctx);
    app.handle(plugin_event(&app, "ca", sign_in_event(None, "THIRD")), &ctx);
    assert!(app.wants_show);
}

#[test]
fn accounts_go_only_to_plugins_that_need_them() {
    let (mut app, mut commands) = app_with_service();
    plugin_entry(&mut app, "ca", &["accounts"]);
    plugin_entry(&mut app, "light", &[]);
    app.sync_plugins();
    let started = sent(&mut commands);
    let accounts_in = |id: &str| {
        started.iter().find_map(|c| match c {
            Command::StartPlugin {
                id: started,
                hello: crate::plugin::protocol::HostMessage::Hello { accounts, .. },
                ..
            } if started == id => Some(accounts.len()),
            _ => None,
        })
    };
    assert_eq!(accounts_in("ca"), Some(1));
    assert_eq!(accounts_in("light"), Some(0));
    // Nothing changed: nothing to tell.
    app.remember_accounts();
    assert!(commands.try_recv().is_err());
    app.auth = AuthStatus::SignedIn {
        account: "u-me.t1".into(),
        name: "Me".into(),
        username: "me@fabrikam.com".into(),
    };
    app.remember_accounts();
    let changed = sent(&mut commands);
    assert!(changed.iter().any(|c| matches!(
        c,
        Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Accounts { accounts } }
            if id == "ca" && accounts[0].username == "me@fabrikam.com"
    )));
    assert!(changed.iter().any(|c| matches!(
        c,
        Command::PluginHello { id, accounts: Some(_), .. } if id == "ca"
    )));
    assert!(
        !changed.iter().any(|c| matches!(
            c,
            Command::PluginSend { id, .. } | Command::PluginHello { id, .. } if id == "light"
        )),
        "a plugin that needs no accounts hears nothing about them"
    );
}

#[test]
fn a_ready_that_asks_for_accounts_gets_them_once() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "new", &[]);
    app.sync_plugins();
    sent(&mut commands);
    app.handle(
        plugin_event(
            &app,
            "new",
            ready_event(&["accounts"], &[], Vec::new(), &[]),
        ),
        &ctx,
    );
    let after = sent(&mut commands);
    assert!(after.iter().any(|c| matches!(
        c,
        Command::PluginSend {
            message: crate::plugin::protocol::HostMessage::Accounts { .. },
            ..
        }
    )));
    assert_eq!(app.settings.plugins[0].needs, vec!["accounts".to_owned()]);
    app.handle(
        plugin_event(
            &app,
            "new",
            ready_event(&["accounts"], &[], Vec::new(), &[]),
        ),
        &ctx,
    );
    assert!(!sent(&mut commands).iter().any(|c| matches!(
        c,
        Command::PluginSend {
            message: crate::plugin::protocol::HostMessage::Accounts { .. },
            ..
        }
    )));
}

#[test]
fn presence_goes_only_to_plugins_that_need_it() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    app.my_presence = Some(busy());
    plugin_entry(&mut app, "light", &[]);
    plugin_entry(&mut app, "other", &[]);
    app.sync_plugins();
    sent(&mut commands);
    app.handle(
        plugin_event(
            &app,
            "light",
            ready_event(&["presence"], &["presence_display"], Vec::new(), &[]),
        ),
        &ctx,
    );
    app.handle(
        plugin_event(
            &app,
            "other",
            ready_event(&[], &["presence_display"], Vec::new(), &[]),
        ),
        &ctx,
    );
    let presences: Vec<Command> = sent(&mut commands)
        .into_iter()
        .filter(|c| {
            matches!(
                c,
                Command::PluginSend {
                    message: crate::plugin::protocol::HostMessage::Presence { .. },
                    ..
                }
            )
        })
        .collect();
    assert_eq!(presences.len(), 1);
    assert!(matches!(
        &presences[0],
        Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Presence { availability, activity } }
            if id == "light" && availability == "Busy" && activity.is_none()
    ));
}

#[test]
fn presence_is_resent_only_when_it_changes() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    app.my_presence = Some(busy());
    plugin_entry(&mut app, "light", &[]);
    app.sync_plugins();
    app.handle(
        plugin_event(
            &app,
            "light",
            ready_event(&["presence"], &[], Vec::new(), &[]),
        ),
        &ctx,
    );
    let count = |commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>| {
        sent(commands)
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Command::PluginSend {
                        message: crate::plugin::protocol::HostMessage::Presence { .. },
                        ..
                    }
                )
            })
            .count()
    };
    assert_eq!(count(&mut commands), 1);
    app.sync_plugin_presence();
    app.handle(Event::MyPresence(busy()), &ctx);
    assert_eq!(count(&mut commands), 0, "the same presence is not resent");
    app.handle(
        Event::MyPresence(Presence {
            availability: crate::model::Availability::Available,
            ..Default::default()
        }),
        &ctx,
    );
    assert_eq!(count(&mut commands), 1);
}

#[test]
fn presence_is_resent_when_the_account_signs_out() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    app.my_presence = Some(busy());
    plugin_entry(&mut app, "light", &[]);
    app.sync_plugins();
    app.handle(
        plugin_event(
            &app,
            "light",
            ready_event(&["presence"], &[], Vec::new(), &[]),
        ),
        &ctx,
    );
    sent(&mut commands);
    app.handle(Event::Auth(AuthStatus::SignedOut), &ctx);
    assert!(sent(&mut commands).iter().any(|c| matches!(
        c,
        Command::PluginSend {
            message: crate::plugin::protocol::HostMessage::Presence { availability, .. },
            ..
        } if availability == "Unknown"
    )));
}

#[test]
fn a_plugin_that_drops_and_regains_the_accounts_need_is_sent_them_again() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "p", &["accounts"]);
    app.sync_plugins();
    app.handle(
        plugin_event(&app, "p", ready_event(&[], &[], Vec::new(), &[])),
        &ctx,
    );
    assert!(app.plugins["p"].sent_accounts.is_none());
    sent(&mut commands);
    app.handle(
        plugin_event(&app, "p", ready_event(&["accounts"], &[], Vec::new(), &[])),
        &ctx,
    );
    assert!(sent(&mut commands).iter().any(|c| matches!(
        c,
        Command::PluginSend {
            message: crate::plugin::protocol::HostMessage::Accounts { .. },
            ..
        }
    )));
}

#[test]
fn a_second_call_alerts_plugin_is_ignored() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    call_alerts_plugin(&mut app, "first");
    call_alerts_plugin(&mut app, "second");
    assert_eq!(app.call_alerts_owner().as_deref(), Some("first"));
    app.handle(
        plugin_event(
            &app,
            "second",
            crate::plugin::event::PluginEvent::IncomingCall(ring("c1", "u-ada")),
        ),
        &ctx,
    );
    app.handle(
        plugin_event(&app, "second", sign_in_event(None, "NO")),
        &ctx,
    );
    assert!(app.rings.is_empty());
    assert!(app.plugins["second"].sign_in.is_empty());
    app.handle(
        plugin_event(
            &app,
            "first",
            crate::plugin::event::PluginEvent::IncomingCall(ring("c1", "u-ada")),
        ),
        &ctx,
    );
    assert_eq!(app.rings.len(), 1);
    // With the first one off, the second takes over.
    app.settings.plugins[0].enabled = false;
    assert_eq!(app.call_alerts_owner().as_deref(), Some("second"));
}

#[test]
fn a_non_owner_call_alerts_plugin_shows_no_sign_in_notices() {
    use crate::plugin::event::PluginEvent;
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    call_alerts_plugin(&mut app, "first");
    call_alerts_plugin(&mut app, "second");
    let toasts = app.toasts.len();
    let account = || Some("u-me.demo".to_owned());
    app.handle(
        plugin_event(&app, "second", PluginEvent::SignedIn { account: account() }),
        &ctx,
    );
    app.handle(
        plugin_event(
            &app,
            "second",
            PluginEvent::Listening {
                account: "u-me.demo".into(),
            },
        ),
        &ctx,
    );
    app.handle(
        plugin_event(
            &app,
            "second",
            PluginEvent::Error {
                account: account(),
                message: "nope".into(),
            },
        ),
        &ctx,
    );
    assert_eq!(app.toasts.len(), toasts);
    assert!(app.plugins["second"].listening.is_empty());
    // The owner still gets its notices.
    app.handle(
        plugin_event(&app, "first", PluginEvent::SignedIn { account: account() }),
        &ctx,
    );
    assert_eq!(app.toasts.len(), toasts + 1);
}

#[test]
fn sign_in_cards_key_on_plugin_and_account() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    for id in ["a", "b"] {
        plugin_entry(&mut app, id, &[]);
        app.plugins.insert(id.into(), PluginState::default());
    }
    app.handle(
        plugin_event(&app, "a", sign_in_event(Some("u-me.demo"), "A1")),
        &ctx,
    );
    app.handle(
        plugin_event(&app, "b", sign_in_event(Some("u-me.demo"), "B1")),
        &ctx,
    );
    app.handle(
        plugin_event(&app, "b", sign_in_event(Some("u-me.demo"), "B2")),
        &ctx,
    );
    assert_eq!(app.plugins["a"].sign_in.len(), 1);
    assert_eq!(app.plugins["b"].sign_in.len(), 1);
    assert_eq!(app.plugins["a"].sign_in[0].user_code, "A1");
    assert_eq!(app.plugins["b"].sign_in[0].user_code, "B2");
    assert_eq!(app.plugins["b"].sign_in[0].plugin, "b");
    app.apply(
        Action::HidePluginSignIn {
            plugin: "a".into(),
            account: Some("u-me.demo".into()),
        },
        &ctx,
    );
    assert!(app.plugins["a"].sign_in.is_empty());
    assert_eq!(app.plugins["b"].sign_in.len(), 1);
}

#[test]
fn removing_a_plugin_deletes_its_secrets_and_stops_it() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "light", &["accounts"]);
    app.settings.plugins[0].secrets = vec!["token".into(), "key".into()];
    app.sync_plugins();
    sent(&mut commands);
    app.apply(Action::RemovePlugin("light".into()), &ctx);
    let after = sent(&mut commands);
    assert!(after.iter().any(|c| matches!(
        c,
        Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Forget { account } }
            if id == "light" && account == "u-me.t1"
    )));
    for option in ["token", "key"] {
        assert!(after.iter().any(|c| matches!(
            c,
            Command::SavePluginSecret { plugin, option: o, value: None, .. }
                if plugin == "light" && o == option
        )));
    }
    assert!(
        after
            .iter()
            .any(|c| matches!(c, Command::StopPlugin { id } if id == "light"))
    );
    assert!(app.settings.plugins.is_empty());
    assert!(!app.plugins.contains_key("light"));
}

#[test]
fn adding_a_plugin_names_it_from_its_file_and_starts_it() {
    let (mut app, mut commands) = app_with_service();
    app.add_plugin(PathBuf::from("C:/x/Chaat Status-Light.exe"));
    app.add_plugin(PathBuf::from("D:/y/chaat status-light.exe"));
    let ids: Vec<&str> = app.settings.plugins.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["chaat-status-light", "chaat-status-light-2"]);
    assert!(app.settings.plugins.iter().all(|e| e.enabled));
    let started = sent(&mut commands);
    assert_eq!(
        started
            .iter()
            .filter(|c| matches!(c, Command::StartPlugin { .. }))
            .count(),
        2
    );
}

#[test]
fn options_are_sent_after_ready_and_on_change() {
    use crate::plugin::protocol::{OptionKind, OptionSpec};
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "light", &[]);
    app.settings.plugins[0]
        .options
        .insert("ip".into(), serde_json::json!("10.0.0.5"));
    app.settings.plugins[0].secrets = vec!["key".into()];
    app.sync_plugins();
    sent(&mut commands);
    let spec = |id: &str, kind, default| OptionSpec {
        id: id.into(),
        kind,
        label: id.into(),
        default,
        choices: Vec::new(),
        ..Default::default()
    };
    let options = vec![
        spec("ip", OptionKind::Text, Some(serde_json::json!(""))),
        spec("flash", OptionKind::Toggle, Some(serde_json::json!(true))),
        spec("key", OptionKind::Secret, None),
    ];
    app.handle(
        plugin_event(&app, "light", ready_event(&[], &[], options, &[])),
        &ctx,
    );
    let values = |commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>| {
        sent(commands).into_iter().find_map(|c| match c {
            Command::PluginOptions {
                id,
                values,
                secrets,
            } if id == "light" => Some((values, secrets)),
            _ => None,
        })
    };
    let (first, secrets) = values(&mut commands).expect("options after ready");
    assert_eq!(first["ip"], serde_json::json!("10.0.0.5"));
    assert_eq!(first["flash"], serde_json::json!(true));
    assert_eq!(secrets, vec!["key".to_owned()]);
    app.apply(
        Action::SetPluginOption {
            plugin: "light".into(),
            option: "flash".into(),
            value: serde_json::json!(false),
        },
        &ctx,
    );
    let (changed, _) = values(&mut commands).expect("options after a change");
    assert_eq!(changed["flash"], serde_json::json!(false));
    assert_eq!(
        app.settings.plugins[0].options["flash"],
        serde_json::json!(false)
    );
    // A cleared secret leaves the list, a stored one joins it, and the
    // plugin hears each time.
    let saved = |set: bool| Event::PluginSecretSaved {
        plugin: "light".into(),
        instance: "inst-light".into(),
        option: "key".into(),
        set,
        ok: true,
    };
    app.handle(saved(false), &ctx);
    let (_, secrets) = values(&mut commands).expect("options after a clear");
    assert!(secrets.is_empty());
    app.handle(saved(true), &ctx);
    let (_, secrets) = values(&mut commands).expect("options after a secret");
    assert_eq!(secrets, vec!["key".to_owned()]);
}

#[test]
fn an_account_error_names_the_account_and_stops_listening() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    call_alerts_plugin(&mut app, "ca");
    app.plugins.get_mut("ca").unwrap().info.name = Some("Call alerts".into());
    app.handle(
        plugin_event(
            &app,
            "ca",
            crate::plugin::event::PluginEvent::Listening {
                account: FABRIKAM.into(),
            },
        ),
        &ctx,
    );
    app.handle(
        plugin_event(
            &app,
            "ca",
            crate::plugin::event::PluginEvent::Error {
                account: Some(FABRIKAM.into()),
                message: "could not find the push service host".into(),
            },
        ),
        &ctx,
    );
    assert!(app.plugins["ca"].listening.is_empty());
    assert!(app.toasts.iter().any(|t| {
        t.text
            .starts_with("Plugin \u{201c}Call alerts\u{201d} for alex@fabrikam.com:")
    }));
}

#[test]
fn signing_out_with_a_plugin_off_still_clears_its_sign_in() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "ca", &["accounts"]);
    app.settings.plugins[0].enabled = false;
    app.apply(Action::SignOut, &ctx);
    // Not running: queued for the next start, which tells the plugin.
    assert_eq!(app.settings.plugins[0].forget, vec!["u-me.t1".to_owned()]);
    assert!(commands.try_recv().is_err());
    app.settings.plugins[0].enabled = true;
    app.sync_plugins();
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::StartPlugin { hello: crate::plugin::protocol::HostMessage::Hello { forget, .. }, .. })
            if forget == vec!["u-me.t1".to_owned()]
    ));
    // Once the plugin is ready, the queue is done.
    app.handle(
        plugin_event(
            &app,
            "ca",
            ready_event(&["accounts"], &[], Vec::new(), &["u-me.t1"]),
        ),
        &ctx,
    );
    assert!(app.settings.plugins[0].forget.is_empty());
    assert!(app.plugins["ca"].running);
}

#[test]
fn a_sign_out_queues_forget_per_plugin() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "a", &["accounts"]);
    plugin_entry(&mut app, "b", &["accounts"]);
    plugin_entry(&mut app, "c", &[]);
    app.sync_plugins();
    sent(&mut commands);
    app.apply(Action::SignOut, &ctx);
    let queue = |id: &str| {
        app.settings
            .plugins
            .iter()
            .find(|e| e.id == id)
            .unwrap()
            .forget
            .clone()
    };
    assert_eq!(queue("a"), vec!["u-me.t1".to_owned()]);
    assert_eq!(queue("b"), vec!["u-me.t1".to_owned()]);
    assert!(queue("c").is_empty());
    let after = sent(&mut commands);
    for id in ["a", "b"] {
        assert!(after.iter().any(|c| matches!(
            c,
            Command::PluginSend { id: to, message: crate::plugin::protocol::HostMessage::Forget { account } }
                if to == id && account == "u-me.t1"
        )));
    }
    assert!(
        !after.iter().any(|c| matches!(
            c,
            Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Forget { .. } } if id == "c"
        )),
        "a plugin that needs no accounts is not told to forget one"
    );
}

#[test]
fn removing_an_account_drops_its_plugin_state() {
    let (mut app, _commands) = app_with_service();
    plugin_entry(&mut app, "ca", &["accounts"]);
    app.plugins.insert(
        "ca".into(),
        PluginState {
            running: true,
            listening: vec!["u-me.t1".into(), "gone.t9".into()],
            sign_in: vec![crate::model::PluginSignIn {
                plugin: "ca".into(),
                account: Some("gone.t9".into()),
                username: None,
                verification_uri: "https://microsoft.com/devicelogin".into(),
                user_code: "OLD".into(),
            }],
            ..Default::default()
        },
    );
    app.remember_accounts();
    assert_eq!(app.plugins["ca"].listening, vec!["u-me.t1".to_owned()]);
    assert!(app.plugins["ca"].sign_in.is_empty());
}

#[test]
fn a_sign_out_stays_queued_until_the_plugin_confirms_or_the_account_returns() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "ca", &["accounts"]);
    app.sync_plugins();
    sent(&mut commands);
    app.apply(Action::SignOut, &ctx);
    let after = sent(&mut commands);
    // Told at once, and still queued in case it never arrives.
    assert!(after.iter().any(|c| matches!(
        c,
        Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Forget { account } }
            if id == "ca" && account == "u-me.t1"
    )));
    assert_eq!(app.settings.plugins[0].forget, vec!["u-me.t1".to_owned()]);
    // A ready that didn't clear it keeps it queued.
    app.handle(
        plugin_event(&app, "ca", ready_event(&["accounts"], &[], Vec::new(), &[])),
        &ctx,
    );
    assert_eq!(app.settings.plugins[0].forget, vec!["u-me.t1".to_owned()]);
    sent(&mut commands);
    // Signing that account in again takes it off the queue.
    app.id = Some("u-me.t1".into());
    app.remember_accounts();
    assert!(app.settings.plugins[0].forget.is_empty());
    assert!(sent(&mut commands).iter().any(|c| matches!(
        c,
        Command::PluginHello { id, forget: Some(forget), .. } if id == "ca" && forget.is_empty()
    )));
}

fn option_spec(
    id: &str,
    kind: crate::plugin::protocol::OptionKind,
    default: Option<serde_json::Value>,
) -> crate::plugin::protocol::OptionSpec {
    crate::plugin::protocol::OptionSpec {
        id: id.into(),
        kind,
        label: id.into(),
        default,
        choices: Vec::new(),
        ..Default::default()
    }
}

/// An enabled entry the person has not decided anything about.
fn undecided_entry(app: &mut App, id: &str) {
    plugin_entry(app, id, &[]);
    let entry = app.settings.plugins.last_mut().unwrap();
    entry.approved_needs.clear();
}

fn accounts_sent_to(commands: &[Command], id: &str) -> bool {
    commands.iter().any(|c| {
        matches!(
            c,
            Command::PluginSend { id: to, message: crate::plugin::protocol::HostMessage::Accounts { .. } }
                if to == id
        )
    })
}

fn presence_sent_to(commands: &[Command], id: &str) -> bool {
    commands.iter().any(|c| {
        matches!(
            c,
            Command::PluginSend { id: to, message: crate::plugin::protocol::HostMessage::Presence { .. } }
                if to == id
        )
    })
}

#[test]
fn events_for_unknown_accounts_are_dropped() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    call_alerts_plugin(&mut app, "ca");
    sent(&mut commands);
    let stranger = Some("u-stranger.t9");
    app.handle(
        plugin_event(&app, "ca", sign_in_event(stranger, "AB12")),
        &ctx,
    );
    app.handle(plugin_event(&app, "ca", signed_in_event(stranger)), &ctx);
    app.handle(
        plugin_event(
            &app,
            "ca",
            crate::plugin::event::PluginEvent::Listening {
                account: "u-stranger.t9".into(),
            },
        ),
        &ctx,
    );
    app.handle(
        plugin_event(
            &app,
            "ca",
            crate::plugin::event::PluginEvent::Error {
                account: stranger.map(str::to_owned),
                message: "nope".into(),
            },
        ),
        &ctx,
    );
    let mut alert = ring("stranger-call", "u-ada");
    alert.account = stranger.map(str::to_owned);
    app.handle(
        plugin_event(
            &app,
            "ca",
            crate::plugin::event::PluginEvent::IncomingCall(alert),
        ),
        &ctx,
    );
    assert!(app.plugins["ca"].sign_in.is_empty());
    assert!(app.plugins["ca"].listening.is_empty());
    assert!(app.toasts.is_empty());
    assert!(app.rings.is_empty());
    // The same events for one of Chaat's accounts count.
    app.handle(
        plugin_event(&app, "ca", sign_in_event(Some("u-me.t1"), "AB12")),
        &ctx,
    );
    app.handle(
        plugin_event(
            &app,
            "ca",
            crate::plugin::event::PluginEvent::Listening {
                account: "u-me.t1".into(),
            },
        ),
        &ctx,
    );
    assert_eq!(app.plugins["ca"].sign_in.len(), 1);
    assert_eq!(app.plugins["ca"].listening, ["u-me.t1"]);
}

#[test]
fn a_new_need_waits_for_approval() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    app.my_presence = Some(busy());
    undecided_entry(&mut app, "light");
    app.sync_plugins();
    sent(&mut commands);
    let ready = |app: &mut App| {
        app.handle(
            plugin_event(
                app,
                "light",
                ready_event(&["accounts", "presence"], &[], Vec::new(), &[]),
            ),
            &ctx,
        );
    };
    ready(&mut app);
    let after = sent(&mut commands);
    assert!(!accounts_sent_to(&after, "light"));
    assert!(!presence_sent_to(&after, "light"));
    assert_eq!(app.plugins["light"].pending_needs, ["accounts", "presence"]);
    assert!(app.plugin_receives("light").is_empty());
    assert_eq!(app.toasts.len(), 1);
    assert_eq!(
        app.toasts[0].text,
        "Plugin \u{201c}light\u{201d} asks for access. Open Settings \u{203a} Plugins to allow it."
    );
    assert!(
        !app.wants_show,
        "asking for access does not raise the window"
    );
    // The same pending set does not announce itself again.
    app.toasts.clear();
    ready(&mut app);
    assert!(app.toasts.is_empty());
    assert!(!accounts_sent_to(&sent(&mut commands), "light"));
}

#[test]
fn approved_needs_flow_and_denied_ones_dont() {
    let ctx = egui::Context::default();
    let ready = |app: &mut App| {
        app.handle(
            plugin_event(
                app,
                "light",
                ready_event(&["accounts", "presence"], &[], Vec::new(), &[]),
            ),
            &ctx,
        );
    };
    let (mut app, mut commands) = app_with_service();
    app.my_presence = Some(busy());
    undecided_entry(&mut app, "light");
    app.sync_plugins();
    ready(&mut app);
    sent(&mut commands);
    app.apply(Action::ApprovePluginNeeds("light".into()), &ctx);
    let after = sent(&mut commands);
    assert!(accounts_sent_to(&after, "light"));
    assert!(presence_sent_to(&after, "light"));
    let entry = &app.settings.plugins[0];
    assert_eq!(entry.approved_needs, ["accounts", "presence"]);
    assert!(entry.denied_needs.is_empty());
    assert!(app.plugins["light"].pending_needs.is_empty());
    assert_eq!(app.plugin_receives("light"), ["accounts", "presence"]);
    // A restart hello carries the accounts of an approved plugin.
    app.settings.plugins[0].path = PathBuf::from("other.exe");
    app.sync_plugins();
    assert!(sent(&mut commands).iter().any(|c| matches!(
        c,
        Command::StartPlugin { hello: crate::plugin::protocol::HostMessage::Hello { accounts, .. }, .. }
            if accounts.len() == 1
    )));

    // A refusal sends nothing, and the same `ready` does not ask again.
    let (mut app, mut commands) = app_with_service();
    app.my_presence = Some(busy());
    undecided_entry(&mut app, "light");
    app.sync_plugins();
    ready(&mut app);
    sent(&mut commands);
    app.apply(Action::DenyPluginNeeds("light".into()), &ctx);
    assert_eq!(
        app.settings.plugins[0].denied_needs,
        ["accounts", "presence"]
    );
    assert!(app.settings.plugins[0].approved_needs.is_empty());
    app.toasts.clear();
    ready(&mut app);
    let after = sent(&mut commands);
    assert!(!accounts_sent_to(&after, "light"));
    assert!(!presence_sent_to(&after, "light"));
    assert!(app.plugins["light"].pending_needs.is_empty());
    assert!(app.toasts.is_empty());
    assert!(app.plugin_receives("light").is_empty());
}

#[test]
fn a_changed_file_waits_and_trust_resets_needs() {
    use crate::plugin::event::PluginEvent;
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "light", &["accounts"]);
    {
        let entry = &mut app.settings.plugins[0];
        entry.sha256 = Some("old".into());
        entry.denied_needs = vec!["presence".into()];
    }
    app.sync_plugins();
    assert!(matches!(
        sent(&mut commands).first(),
        Some(Command::StartPlugin { expected_sha256: Some(pin), .. }) if pin == "old"
    ));
    app.handle(
        plugin_event(
            &app,
            "light",
            PluginEvent::FileChanged {
                sha256: "new".into(),
            },
        ),
        &ctx,
    );
    assert_eq!(app.plugins["light"].file_changed.as_deref(), Some("new"));
    assert!(!app.plugins["light"].running);
    assert!(
        sent(&mut commands).is_empty(),
        "the changed file is not run"
    );
    app.sync_plugins();
    assert!(sent(&mut commands).is_empty(), "nor restarted by a sync");

    app.apply(Action::TrustPluginFile("light".into()), &ctx);
    let entry = &app.settings.plugins[0];
    assert_eq!(entry.sha256.as_deref(), Some("new"));
    assert!(entry.approved_needs.is_empty() && entry.denied_needs.is_empty());
    assert!(app.plugins["light"].file_changed.is_none());
    assert!(matches!(
        sent(&mut commands).first(),
        Some(Command::StartPlugin { expected_sha256: Some(pin), hello: crate::plugin::protocol::HostMessage::Hello { accounts, .. }, .. })
            if pin == "new" && accounts.is_empty()
    ));

    // Keeping it off turns the plugin off and drops the question.
    app.handle(
        plugin_event(
            &app,
            "light",
            PluginEvent::FileChanged {
                sha256: "newer".into(),
            },
        ),
        &ctx,
    );
    app.apply(Action::KeepPluginOff("light".into()), &ctx);
    assert!(!app.settings.plugins[0].enabled);
    assert!(app.plugins["light"].file_changed.is_none());
    assert_eq!(app.settings.plugins[0].sha256.as_deref(), Some("new"));
    assert!(sent(&mut commands).iter().any(|c| matches!(
        c,
        Command::StopPlugin { id } if id == "light"
    )));
}

#[test]
fn the_first_fingerprint_is_stored_and_never_replaced() {
    use crate::plugin::event::PluginEvent;
    let (mut app, _commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "light", &[]);
    let fingerprint = |app: &mut App, sha: &str| {
        app.handle(
            plugin_event(
                app,
                "light",
                PluginEvent::Fingerprint { sha256: sha.into() },
            ),
            &ctx,
        );
    };
    fingerprint(&mut app, "first");
    assert_eq!(app.settings.plugins[0].sha256.as_deref(), Some("first"));
    fingerprint(&mut app, "second");
    assert_eq!(app.settings.plugins[0].sha256.as_deref(), Some("first"));
}

#[test]
fn a_late_secret_answer_for_an_old_instance_is_ignored() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "light", &[]);
    app.settings.plugins[0].instance = "new".into();
    app.plugins.entry("light".into()).or_default().options.push(
        crate::plugin::protocol::OptionSpec {
            id: "key".into(),
            kind: crate::plugin::protocol::OptionKind::Secret,
            label: "Key".into(),
            ..Default::default()
        },
    );
    let answer = |instance: &str| Event::PluginSecretSaved {
        plugin: "light".into(),
        instance: instance.into(),
        option: "key".into(),
        set: true,
        ok: true,
    };
    app.handle(answer("old"), &ctx);
    assert!(app.settings.plugins[0].secrets.is_empty());
    assert!(sent(&mut commands).is_empty());
    app.handle(answer("new"), &ctx);
    assert_eq!(app.settings.plugins[0].secrets, ["key"]);
    // Saves carry the entry's instance.
    app.apply(
        Action::SetPluginSecret {
            plugin: "light".into(),
            option: "key".into(),
            value: Some("v".into()),
        },
        &ctx,
    );
    assert!(sent(&mut commands).iter().any(|c| matches!(
        c,
        Command::SavePluginSecret { instance, .. } if instance == "new"
    )));
    // Each new entry gets its own instance.
    app.add_plugin(PathBuf::from("C:/x/other.exe"));
    app.add_plugin(PathBuf::from("D:/y/other.exe"));
    let instances: Vec<&str> = app
        .settings
        .plugins
        .iter()
        .skip(1)
        .map(|e| e.instance.as_str())
        .collect();
    assert!(instances.iter().all(|i| i.len() == 32));
    assert_ne!(instances[0], instances[1]);
}

#[test]
fn removing_deletes_every_described_secret() {
    use crate::plugin::protocol::OptionKind;
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "light", &[]);
    app.settings.plugins[0].secrets = vec!["recorded".into()];
    app.sync_plugins();
    sent(&mut commands);
    app.handle(
        plugin_event(
            &app,
            "light",
            ready_event(
                &[],
                &[],
                vec![
                    option_spec("recorded", OptionKind::Secret, None),
                    option_spec("described", OptionKind::Secret, None),
                    option_spec("plain", OptionKind::Text, None),
                ],
                &[],
            ),
        ),
        &ctx,
    );
    sent(&mut commands);
    app.apply(Action::RemovePlugin("light".into()), &ctx);
    let deleted: Vec<String> = sent(&mut commands)
        .into_iter()
        .filter_map(|c| match c {
            Command::SavePluginSecret {
                plugin,
                instance,
                option,
                value: None,
            } if plugin == "light" && instance == "inst-light" => Some(option),
            _ => None,
        })
        .collect();
    assert_eq!(deleted.len(), 2);
    assert!(deleted.contains(&"recorded".to_owned()));
    assert!(deleted.contains(&"described".to_owned()));
}

#[test]
fn forget_needs_the_accounts_need() {
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "allowed", &["accounts"]);
    plugin_entry(&mut app, "unallowed", &["accounts"]);
    app.settings.plugins[1].approved_needs.clear();
    for entry in &mut app.settings.plugins {
        entry.forget = vec!["u-queued.t2".into()];
    }
    app.sync_plugins();
    let started = sent(&mut commands);
    let hello_forget = |id: &str| {
        started.iter().find_map(|c| match c {
            Command::StartPlugin {
                id: started,
                hello: crate::plugin::protocol::HostMessage::Hello { forget, .. },
                ..
            } if started == id => Some(forget.clone()),
            _ => None,
        })
    };
    assert_eq!(
        hello_forget("allowed"),
        Some(vec!["u-queued.t2".to_owned()])
    );
    assert_eq!(hello_forget("unallowed"), Some(Vec::new()));
    app.apply(Action::SignOut, &ctx);
    let after = sent(&mut commands);
    let forgets = |id: &str| {
        after
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Command::PluginSend { id: to, message: crate::plugin::protocol::HostMessage::Forget { .. } }
                        if to == id
                )
            })
            .count()
    };
    assert_eq!(forgets("allowed"), 1);
    assert_eq!(forgets("unallowed"), 0, "an unallowed plugin is not told");
    assert!(
        !app.settings.plugins[1]
            .forget
            .contains(&"u-me.t1".to_owned()),
        "nor is the account queued for it"
    );
    assert!(
        app.settings.plugins[0]
            .forget
            .contains(&"u-me.t1".to_owned())
    );
}

#[test]
fn options_follow_the_current_schema() {
    use crate::plugin::protocol::OptionKind;
    let (mut app, mut commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "light", &[]);
    {
        let entry = &mut app.settings.plugins[0];
        entry
            .options
            .insert("ip".into(), serde_json::json!("10.0.0.5"));
        entry.options.insert("old".into(), serde_json::json!(1));
        entry
            .options
            .insert("was_plain".into(), serde_json::json!("leaked"));
        entry.secrets = vec!["key".into(), "gone".into(), "ip".into()];
    }
    app.sync_plugins();
    sent(&mut commands);
    app.handle(
        plugin_event(
            &app,
            "light",
            ready_event(
                &[],
                &[],
                vec![
                    option_spec("ip", OptionKind::Text, None),
                    option_spec("was_plain", OptionKind::Secret, None),
                    option_spec("key", OptionKind::Secret, None),
                ],
                &[],
            ),
        ),
        &ctx,
    );
    let (values, secrets) = sent(&mut commands)
        .into_iter()
        .find_map(|c| match c {
            Command::PluginOptions {
                id,
                values,
                secrets,
            } if id == "light" => Some((values, secrets)),
            _ => None,
        })
        .expect("options after ready");
    assert_eq!(
        values.keys().collect::<Vec<_>>(),
        ["ip"],
        "only described plain options"
    );
    assert_eq!(secrets, ["key"], "only secret options with a stored value");
    let stored = &app.settings.plugins[0].options;
    assert!(
        !stored.contains_key("was_plain"),
        "a plain value for an option that is now secret is deleted"
    );
    assert!(stored.contains_key("old"), "unknown options are withheld");
    assert!(stored.contains_key("ip"));
}

#[test]
fn plugin_toasts_are_quoted_and_limited() {
    let (mut app, _commands) = app_with_service();
    let ctx = egui::Context::default();
    plugin_entry(&mut app, "light", &[]);
    app.plugins.entry("light".into()).or_default().info.name = Some("Busy \u{201d}light".into());
    let stopped = |app: &mut App, id: &str, message: &str| {
        app.handle(
            plugin_event(
                app,
                id,
                crate::plugin::event::PluginEvent::Status {
                    running: false,
                    message: Some(message.into()),
                },
            ),
            &ctx,
        );
    };
    stopped(&mut app, "light", "first");
    assert_eq!(app.toasts.len(), 1);
    assert_eq!(
        app.toasts[0].text,
        "Plugin \u{201c}Busy 'light\u{201d}: first"
    );
    stopped(&mut app, "light", "second");
    assert_eq!(app.toasts.len(), 1, "a second toast within 10 s is dropped");
    // Another plugin is not held back by this one.
    plugin_entry(&mut app, "other", &[]);
    stopped(&mut app, "other", "hi");
    assert_eq!(app.toasts.len(), 2);
    // After the gap, the plugin may toast again.
    if let Some(past) = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(11)) {
        app.plugin_toasts.insert("light".into(), past);
        stopped(&mut app, "light", "third");
        assert!(app.toasts.iter().any(|t| t.text.ends_with(": third")));
    }
}
