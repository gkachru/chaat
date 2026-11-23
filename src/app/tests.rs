use super::*;
use crate::backend::Command;
use crate::html::RichText;
use crate::plugin::registry::{Capability, Need};
#[test]
fn inactive_conversations_are_bounded_but_unsent_messages_and_drafts_survive() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    let unsent = ConversationId::Chat("unsent".into());
    let mut pending = message("pending", 1);
    pending.delivery = Delivery::Failed;
    app.conversations
        .entry(unsent.clone())
        .or_default()
        .messages
        .push(pending);
    app.recent_conversations.push_back(unsent.clone());
    let thread = ConversationId::Channel {
        team: "team".into(),
        channel: "channel".into(),
    };
    let mut root = message("root", 1);
    let mut reply = message("reply", 2);
    reply.delivery = Delivery::Sending;
    root.replies.push(reply);
    app.conversations
        .entry(thread.clone())
        .or_default()
        .messages
        .push(root);
    app.recent_conversations.push_back(thread.clone());
    let draft_id = ConversationId::Chat("draft".into());
    app.drafts.entry(draft_id.clone()).or_default().text = "Keep this".into();
    for i in 0..30 {
        let id = ConversationId::Chat(format!("chat-{i}"));
        app.open_conversation(id.clone());
        app.on_messages(
            id.clone(),
            vec![message("one", i)],
            Merge::Latest,
            Some(true),
            true,
        );
    }
    assert!(app.conversations.len() <= 12);
    assert!(app.conversations.contains_key(&unsent));
    assert!(app.conversations.contains_key(&thread));
    assert_eq!(app.drafts[&draft_id].text, "Keep this");
    let old = ConversationId::Chat("chat-0".into());
    assert!(!app.conversations.contains_key(&old));
    while commands.try_recv().is_ok() {}
    app.apply(Action::OpenConversation(old.clone()), &ctx);
    assert!(
        matches!(commands.try_recv().unwrap(), Command::Account(AccountCommand::CachedMessages(id)) if id == old)
    );
}

#[test]
fn inactive_history_budget_counts_channel_replies() {
    let (mut app, _) = app();
    let heavy = ConversationId::Channel {
        team: "team".into(),
        channel: "channel".into(),
    };
    let mut root = message("root", 1);
    root.replies = (0..2_001).map(|i| message(&i.to_string(), i)).collect();
    app.conversations
        .entry(heavy.clone())
        .or_default()
        .messages
        .push(root);
    app.recent_conversations.push_front(heavy.clone());
    let active = ConversationId::Chat("active".into());
    app.open = Some(active.clone());
    app.conversations.entry(active.clone()).or_default();
    app.trim_conversations();
    assert!(!app.conversations.contains_key(&heavy));
    assert!(app.conversations.contains_key(&active));
}

#[test]
fn a_late_disk_reopen_does_not_replace_fresh_messages() {
    let (mut app, _) = app();
    let chat = ConversationId::Chat("chat".into());
    let old = message("one", 1);
    let mut fresh = old.clone();
    fresh.body = RichText::plain("Edited on Microsoft Graph");
    app.on_messages(chat.clone(), vec![fresh], Merge::Latest, Some(false), false);
    app.on_messages(chat.clone(), vec![old], Merge::Latest, Some(true), true);
    let view = app.conversation(&chat).unwrap();
    assert_eq!(
        view.messages[0].body.to_plain(),
        "Edited on Microsoft Graph"
    );
    assert!(!view.more);
}

fn message(id: &str, created: i64) -> Message {
    Message {
        id: id.into(),
        created,
        body: RichText::plain(id),
        ..Default::default()
    }
}

#[test]
fn thread_replies_are_asked_once_a_minute_and_keep_unsent_ones() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    while commands.try_recv().is_ok() {}
    let channel = ConversationId::Channel {
        team: "t".into(),
        channel: "c".into(),
    };
    let load = || Action::LoadReplies {
        conversation: channel.clone(),
        message: "root".into(),
    };
    app.apply(load(), &ctx);
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Account(AccountCommand::LoadReplies { root, .. }) if root == "root"
    ));
    app.apply(load(), &ctx);
    assert!(commands.try_recv().is_err(), "reused within a minute");

    let message = |id: &str, delivery: Delivery| Message {
        id: id.into(),
        delivery,
        ..Message::default()
    };
    let mut root = message("root", Delivery::Sent);
    root.replies = vec![
        message("old", Delivery::Sent),
        message("local-1", Delivery::Sending),
        message("local-2", Delivery::Failed),
    ];
    app.conversations.get_mut(&channel).unwrap().messages = vec![root];
    app.handle(
        AccountEvent::Replies {
            conversation: channel.clone(),
            root: "root".into(),
            replies: vec![
                message("old", Delivery::Sent),
                message("new", Delivery::Sent),
            ],
        },
        &ctx,
    );
    let ids: Vec<String> = app.conversations[&channel].messages[0]
        .replies
        .iter()
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(ids, ["old", "new", "local-1", "local-2"]);
}

#[test]
fn photos_are_asked_once_while_loading_and_never_in_the_demo() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    while commands.try_recv().is_ok() {}
    app.apply(Action::LoadPhotos(vec!["u-1".into()]), &ctx);
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Account(AccountCommand::Photo(id)) if id == "u-1"
    ));
    assert!(matches!(app.photos.get("u-1"), Some(Image::Loading)));
    app.apply(Action::LoadPhotos(vec!["u-1".into()]), &ctx);
    assert!(commands.try_recv().is_err(), "not asked while loading");

    let (mut demo, _dir) = demo_app();
    demo.apply(Action::LoadPhotos(vec!["u-2".into()]), &ctx);
    assert!(!demo.photos.contains_key("u-2"), "the demo asks nobody");
}

#[test]
fn names_graph_left_out_are_looked_up_once() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    while commands.try_recv().is_ok() {}
    app.apply(Action::LookUpNames(vec!["u-gone".into()]), &ctx);
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Account(AccountCommand::UserName(id)) if id == "u-gone"
    ));
    assert!(!app.needs_name("u-gone"), "asked already");
    app.apply(Action::LookUpNames(vec!["u-gone".into()]), &ctx);
    assert!(commands.try_recv().is_err(), "not asked twice");
    app.handle(
        AccountEvent::UserName {
            user: "u-gone".into(),
            name: Some("Sourabh".into()),
        },
        &ctx,
    );
    assert_eq!(app.person_name("u-gone").as_deref(), Some("Sourabh"));
    assert_eq!(app.person_name(app.me()).as_deref(), Some("You"));
}

#[test]
fn retrying_the_relay_reconnects_only_a_failed_connection() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    while commands.try_recv().is_ok() {}
    app.settings.relay_url = Some("https://relay.example.com".into());
    app.push = crate::realtime::PushStatus::Live;
    app.apply(Action::RetryRelay, &ctx);
    assert!(commands.try_recv().is_err(), "a live connection stays");
    app.push = crate::realtime::PushStatus::Failed("timed out".into());
    app.apply(Action::RetryRelay, &ctx);
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Account(AccountCommand::Relay(Some(url))) if url == "https://relay.example.com"
    ));
}

#[test]
fn the_last_read_divider_skips_own_and_system_messages() {
    let from = |id: &str, at, author: &str, system| Message {
        author_id: Some(author.into()),
        system,
        ..message(id, at)
    };
    let mut view = Conversation {
        messages: vec![
            from("a", 10, "u-ada", false),
            from("b", 20, "u-me", false),
            from("c", 30, "u-ada", true),
            from("d", 40, "u-ada", false),
        ],
        ..Default::default()
    };
    assert_eq!(view.place_divider("u-me"), None, "no divider by default");
    view.set_unread_after(Some(10));
    assert_eq!(view.place_divider("u-me"), Some(3));
    view.set_unread_after(Some(40));
    assert_eq!(view.place_divider("u-me"), None, "nothing newer");
}

#[test]
fn a_moving_divider_measures_its_old_and_new_rows_again() {
    let from = |id: &str, at| Message {
        author_id: Some("u-ada".into()),
        ..message(id, at)
    };
    // More was unread than one page: the divider starts at the oldest
    // loaded message.
    let mut view = Conversation {
        messages: vec![from("c", 30), from("d", 40)],
        unread_after: Some(5),
        ..Default::default()
    };
    assert_eq!(view.place_divider("u-me"), Some(0));
    for id in ["c", "d"] {
        view.heights.insert(id.into(), 50.0);
    }
    // Older history arrives and the divider moves up to it.
    view.messages.splice(0..0, [from("a", 10), from("b", 20)]);
    view.heights.insert("a".into(), 50.0);
    assert_eq!(view.place_divider("u-me"), Some(0));
    assert!(
        !view.heights.contains_key("c"),
        "the old row loses the divider"
    );
    assert!(!view.heights.contains_key("a"), "the new row gains it");
    assert!(
        view.heights.contains_key("d"),
        "other rows keep their height"
    );
}

pub(crate) fn app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
    let (backend, commands) = Backend::recording();
    let dir = std::env::temp_dir().join("chaat-test-unused");
    let mut app = App::with_backend(
        AppDirs::under(&dir),
        Settings::default(),
        Waker::default(),
        backend,
        false,
    );
    app.auth = AuthStatus::SignedIn {
        account: "u-me.t1".into(),
        name: "Me".into(),
        username: "me@contoso.com".into(),
    };
    (app, commands)
}

/// The next command, skipping the outbox copy a send saves first.
fn next_command(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Command {
    loop {
        match commands.try_recv().unwrap() {
            Command::Account(AccountCommand::SaveOutbox(_)) => continue,
            other => return other,
        }
    }
}

pub(crate) fn ring(id: &str, caller: &str) -> crate::calling::RingAlert {
    crate::calling::RingAlert {
        id: id.into(),
        caller: "Caller".into(),
        caller_object_id: Some(caller.to_owned()),
        video: false,
        group: false,
        thread_id: None,
        account: None,
        username: None,
        ring_timeout: None,
    }
}

pub(crate) fn plugin_event(
    app: &App,
    id: &str,
    event: crate::plugin::event::PluginEvent,
) -> ServiceEvent {
    ServiceEvent::Plugin {
        id: id.into(),
        generation: app.plugin_generations.get(id).copied().unwrap_or(0),
        event,
    }
}

pub(crate) fn sign_in_event(
    account: Option<&str>,
    code: &str,
) -> crate::plugin::event::PluginEvent {
    crate::plugin::event::PluginEvent::SignIn {
        account: account.map(str::to_owned),
        verification_uri: "https://microsoft.com/devicelogin".into(),
        user_code: code.into(),
    }
}

pub(crate) fn signed_in_event(account: Option<&str>) -> crate::plugin::event::PluginEvent {
    crate::plugin::event::PluginEvent::SignedIn {
        account: account.map(str::to_owned),
    }
}

pub(crate) fn ready_event(
    needs: &[Need],
    offers: &[Capability],
    options: Vec<crate::plugin::protocol::OptionSpec>,
    forgotten: &[&str],
) -> crate::plugin::event::PluginEvent {
    crate::plugin::event::PluginEvent::Ready {
        info: Default::default(),
        offers: offers.to_vec(),
        needs: needs.to_vec(),
        options,
        forgotten: forgotten.iter().map(|id| (*id).to_owned()).collect(),
    }
}

/// An enabled entry in the plugin list, not started, that the person
/// already allowed everything to.
pub(crate) fn plugin_entry(app: &mut App, id: &str, needs: &[&str]) {
    app.settings.plugins.push(crate::settings::PluginEntry {
        id: id.into(),
        path: PathBuf::from(format!("{id}.exe")),
        enabled: true,
        needs: needs.iter().map(|n| (*n).to_owned()).collect(),
        approved_needs: vec!["accounts".into(), "presence".into()],
        instance: format!("inst-{id}"),
        ..Default::default()
    });
}

/// A running call-alerts plugin, as after its `ready`.
pub(crate) fn call_alerts_plugin(app: &mut App, id: &str) {
    plugin_entry(app, id, &["accounts"]);
    app.plugins.insert(
        id.into(),
        PluginState {
            running: true,
            offers: vec![Capability::CallAlerts],
            needs: vec![Need::Accounts],
            ..Default::default()
        },
    );
}

pub(crate) fn sent(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
    std::iter::from_fn(|| commands.try_recv().ok()).collect()
}

pub(crate) const FABRIKAM: &str = "u-alex2.fabrikam";

pub(crate) const LAUNCH: &str = "19:demo-launch@thread.v2";

pub(crate) fn demo_app() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        AppDirs::under(dir.path()),
        Settings::default(),
        Waker::default(),
        true,
    );
    // The demo's own plugins would own call alerts; tests set up theirs.
    app.settings.plugins.clear();
    app.plugins.clear();
    (app, dir)
}

#[test]
fn in_account_restores_active_and_hidden() {
    let (mut app, _dir) = demo_app();
    assert!(app.accounts.len() >= 2);
    app.active = 0;
    app.hidden = false;
    app.in_account(1, |app| {
        assert_eq!(app.active, 1);
        assert!(app.hidden);
        // Nested inside a hidden account, hidden stays true.
        app.in_account(0, |app| {
            assert_eq!(app.active, 0);
            assert!(app.hidden);
        });
        assert_eq!(app.active, 1);
        assert!(app.hidden);
    });
    assert_eq!(app.active, 0);
    assert!(!app.hidden);
    app.in_account(0, |app| assert!(!app.hidden));
}

fn launch_members(app: &App) -> Vec<String> {
    app.group_members(LAUNCH)
        .into_iter()
        .map(|m| m.user_id)
        .collect()
}

fn person(id: &str) -> Member {
    Member {
        user_id: id.into(),
        display_name: id.into(),
        ..Default::default()
    }
}

/// A signed-in, non-demo app with one group chat of me and Ada.
fn group_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
    let (mut app, commands) = app();
    app.granted = "User.Read Chat.ReadWrite ChatMember.ReadWrite".into();
    app.chats = vec![Chat {
        id: "19:g".into(),
        kind: ChatKind::Group,
        members: vec![person("u-me"), person("u-ada")],
        ..Default::default()
    }];
    (app, commands)
}

#[test]
fn group_members_lists_me_first_then_by_name() {
    let (app, _dir) = demo_app();
    assert_eq!(
        launch_members(&app),
        ["u-me", "u-ada", "u-grace", "u-katherine", "u-margaret"]
    );
    assert!(app.group_members("unknown").is_empty());
}

#[test]
fn open_add_people_resets_scratch() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    app.add_people.picked.push(person("u-alan"));
    app.add_people.history_step = true;
    app.apply(Action::OpenAddPeople(LAUNCH.into()), &ctx);
    assert_eq!(app.overlay, Some(Overlay::AddPeople(LAUNCH.into())));
    assert!(app.add_people.picked.is_empty());
    assert!(!app.add_people.history_step);
}

#[test]
fn add_members_in_demo_appends() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    app.apply(Action::OpenAddPeople(LAUNCH.into()), &ctx);
    app.apply(
        Action::AddMembers {
            chat: LAUNCH.into(),
            members: vec![person("u-alan")],
            history: true,
        },
        &ctx,
    );
    let ids = launch_members(&app);
    assert_eq!(ids.len(), 6);
    assert!(ids.contains(&"u-alan".to_owned()));
    assert_eq!(app.overlay, None);
}

#[test]
fn add_skips_existing_members() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    app.apply(
        Action::AddMembers {
            chat: LAUNCH.into(),
            members: vec![person("u-ada")],
            history: false,
        },
        &ctx,
    );
    assert_eq!(launch_members(&app).len(), 5);
}

#[test]
fn leave_in_demo_removes_chat() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    app.apply(
        Action::OpenConversation(ConversationId::Chat(LAUNCH.into())),
        &ctx,
    );
    app.apply(Action::LeaveChat(LAUNCH.into()), &ctx);
    assert!(app.chats.iter().all(|c| c.id != LAUNCH));
    assert_eq!(app.open, None);
    assert_eq!(app.overlay, None);
}

#[test]
fn adding_and_leaving_work_on_a_meeting_chat_in_demo() {
    const MEETING: &str = "19:meeting_demo-design-review@thread.v2";
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    let before = app.group_members(MEETING).len();
    assert!(before > 0);
    app.apply(
        Action::AddMembers {
            chat: MEETING.into(),
            members: vec![person("u-alan")],
            history: false,
        },
        &ctx,
    );
    assert_eq!(app.group_members(MEETING).len(), before + 1);
    app.apply(
        Action::OpenConversation(ConversationId::Chat(MEETING.into())),
        &ctx,
    );
    app.apply(Action::LeaveChat(MEETING.into()), &ctx);
    assert!(app.chats.iter().all(|c| c.id != MEETING));
    assert_eq!(app.open, None);
}

#[test]
fn open_leave_sets_overlay() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    app.apply(Action::OpenLeaveChat(LAUNCH.into()), &ctx);
    assert_eq!(app.overlay, Some(Overlay::LeaveChat(LAUNCH.into())));
}

#[test]
fn leave_without_scope_shows_consent_dialog() {
    // The dialog itself decides between confirming and explaining.
    let (mut app, _commands) = group_app();
    app.granted = "User.Read Chat.ReadWrite".into();
    assert!(!app.has_scope(crate::auth::CHAT_MEMBER_SCOPE));
    let ctx = egui::Context::default();
    app.apply(Action::OpenLeaveChat("19:g".into()), &ctx);
    assert_eq!(app.overlay, Some(Overlay::LeaveChat("19:g".into())));
}

#[test]
fn leave_without_scope_never_sends() {
    let (mut app, mut commands) = group_app();
    app.granted = "User.Read Chat.ReadWrite".into();
    let ctx = egui::Context::default();
    app.apply(Action::LeaveChat("19:g".into()), &ctx);
    assert_eq!(app.chats.len(), 1);
    assert!(app.leaving.is_empty());
    assert!(commands.try_recv().is_err());
    assert_eq!(app.overlay, Some(Overlay::LeaveChat("19:g".into())));
}

#[test]
fn left_chat_stays_out_until_graph_stops_listing_it() {
    let (mut app, _commands) = group_app();
    let ctx = egui::Context::default();
    app.apply(Action::LeaveChat("19:g".into()), &ctx);
    app.handle(AccountEvent::LeftChat("19:g".into()), &ctx);
    app.handle(
        AccountEvent::Chats {
            chats: vec![Chat {
                id: "19:g".into(),
                ..Default::default()
            }],
            cached: false,
        },
        &ctx,
    );
    assert!(app.chats.is_empty(), "a lagging list must not re-add it");
    app.handle(
        AccountEvent::Chats {
            chats: vec![],
            cached: false,
        },
        &ctx,
    );
    assert!(app.left.is_empty());
}

#[test]
fn people_opened_fetches_members() {
    let (mut app, mut commands) = group_app();
    let ctx = egui::Context::default();
    app.apply(Action::PeopleOpened("19:g".into()), &ctx);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::Fetch {
            resource: Resource::Members(chat),
            trigger: Trigger::Opened,
        })) if chat == "19:g"
    ));
}

#[test]
fn members_event_replaces_members() {
    let (mut app, _commands) = group_app();
    let ctx = egui::Context::default();
    app.handle(
        AccountEvent::Members {
            chat: "19:g".into(),
            members: vec![person("u-me"), person("u-ada"), person("u-grace")],
        },
        &ctx,
    );
    assert_eq!(app.chats[0].members.len(), 3);
    // People still being added stay until Graph confirms them.
    app.apply(
        Action::AddMembers {
            chat: "19:g".into(),
            members: vec![person("u-alan")],
            history: false,
        },
        &ctx,
    );
    app.handle(
        AccountEvent::Members {
            chat: "19:g".into(),
            members: vec![person("u-me")],
        },
        &ctx,
    );
    let ids: Vec<&str> = app.chats[0]
        .members
        .iter()
        .map(|m| m.user_id.as_str())
        .collect();
    assert_eq!(ids, ["u-me", "u-alan"]);
}

#[test]
fn add_members_marks_adding_and_sends() {
    let (mut app, mut commands) = group_app();
    let ctx = egui::Context::default();
    app.apply(
        Action::AddMembers {
            chat: "19:g".into(),
            members: vec![person("u-alan"), person("u-ada")],
            history: true,
        },
        &ctx,
    );
    assert!(
        app.adding
            .contains(&("19:g".to_owned(), "u-alan".to_owned()))
    );
    assert_eq!(app.adding.len(), 1, "existing members are skipped");
    assert_eq!(app.chats[0].members.len(), 3);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::AddMembers { chat, users, history: true }))
            if chat == "19:g" && users == ["u-alan"]
    ));
}

#[test]
fn members_added_clears_adding_and_toasts_failures() {
    let (mut app, _commands) = group_app();
    let ctx = egui::Context::default();
    app.apply(
        Action::AddMembers {
            chat: "19:g".into(),
            members: vec![person("u-alan"), person("u-hedy")],
            history: false,
        },
        &ctx,
    );
    app.handle(
        AccountEvent::MembersAdded {
            chat: "19:g".into(),
            added: vec!["u-alan".into()],
            failed: vec!["u-hedy".into()],
        },
        &ctx,
    );
    assert!(app.adding.is_empty());
    let ids: Vec<&str> = app.chats[0]
        .members
        .iter()
        .map(|m| m.user_id.as_str())
        .collect();
    assert_eq!(ids, ["u-me", "u-ada", "u-alan"]);
    assert!(
        app.toasts
            .iter()
            .any(|t| t.kind == ToastKind::Error && t.text == "Could not add 1 person")
    );
}

#[test]
fn leave_hides_chat_and_sends() {
    let (mut app, mut commands) = group_app();
    let ctx = egui::Context::default();
    app.apply(
        Action::OpenConversation(ConversationId::Chat("19:g".into())),
        &ctx,
    );
    while commands.try_recv().is_ok() {}
    app.apply(Action::LeaveChat("19:g".into()), &ctx);
    assert!(app.chats.is_empty());
    assert!(app.leaving.contains_key("19:g"));
    assert_eq!(app.open, None);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::LeaveChat(chat))) if chat == "19:g"
    ));
    // A chat list refresh does not bring it back meanwhile.
    app.handle(
        AccountEvent::Chats {
            chats: vec![Chat {
                id: "19:g".into(),
                ..Default::default()
            }],
            cached: false,
        },
        &ctx,
    );
    assert!(app.chats.is_empty());
}

#[test]
fn leave_failed_restores_chat() {
    let (mut app, _commands) = group_app();
    let ctx = egui::Context::default();
    app.apply(Action::LeaveChat("19:g".into()), &ctx);
    app.handle(
        AccountEvent::LeaveFailed {
            chat: "19:g".into(),
            admin: false,
        },
        &ctx,
    );
    assert_eq!(app.chats.len(), 1);
    assert!(app.leaving.is_empty());
    assert!(app.toasts.iter().any(|t| t.kind == ToastKind::Error));

    app.apply(Action::LeaveChat("19:g".into()), &ctx);
    app.handle(
        AccountEvent::LeaveFailed {
            chat: "19:g".into(),
            admin: true,
        },
        &ctx,
    );
    assert_eq!(app.chats.len(), 1);
    assert!(
        app.toasts
            .iter()
            .any(|t| t.text.contains("administrator must approve"))
    );
}

#[test]
fn left_chat_forgets_copy() {
    let (mut app, _commands) = group_app();
    let ctx = egui::Context::default();
    app.apply(Action::LeaveChat("19:g".into()), &ctx);
    app.handle(AccountEvent::LeftChat("19:g".into()), &ctx);
    assert!(app.leaving.is_empty());
    assert!(app.chats.is_empty());
}

#[test]
fn latest_pages_connect_to_older_ones() {
    let mut conversation = Conversation::default();
    conversation.merge(
        (0..5).map(|i| message(&i.to_string(), i)).collect(),
        Merge::Latest,
    );
    conversation.merge(
        vec![message("3", 3), message("4", 4), message("5", 5)],
        Merge::Latest,
    );
    let ids: Vec<&str> = conversation
        .messages
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, ["0", "1", "2", "3", "4", "5"]);
    // A page that does not connect replaces the gap-y history.
    conversation.merge(vec![message("9", 9)], Merge::Latest);
    assert_eq!(conversation.messages.len(), 1);
}

#[test]
fn pasted_images_send_without_text_and_survive_retry() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    let chat = ConversationId::Chat("19:images".into());
    let image = crate::clipboard::encode_rgba(1, 1, &[0, 128, 255, 255]).unwrap();
    app.handle(
        AccountEvent::Pasted {
            conversation: chat.clone(),
            range: (0, 0),
            text: String::new(),
            image: Ok(Some(image.clone())),
        },
        &ctx,
    );
    assert_eq!(app.drafts[&chat].images.len(), 1);
    app.apply(
        Action::Send {
            conversation: chat.clone(),
            text: String::new(),
            reply_to: None,
            mentions: Vec::new(),
            files: Vec::new(),
            images: vec![image.clone()],
        },
        &ctx,
    );
    let Command::Account(AccountCommand::Send(crate::outbox::SendPayload {
        local_id, images, ..
    })) = next_command(&mut commands)
    else {
        panic!("send expected")
    };
    assert_eq!(images.as_slice(), std::slice::from_ref(&image));
    assert_eq!(
        app.conversations[&chat].messages[0].body.images().count(),
        1
    );
    app.handle(
        AccountEvent::SendFailed {
            conversation: chat.clone(),
            local_id: local_id.clone(),
            reason: "offline".into(),
        },
        &ctx,
    );
    app.apply(
        Action::RetrySend {
            conversation: chat,
            local_id,
        },
        &ctx,
    );
    let Command::Account(AccountCommand::Send(crate::outbox::SendPayload { images, .. })) =
        commands.try_recv().unwrap()
    else {
        panic!("retry expected")
    };
    assert_eq!(images, [image]);
}

#[test]
fn text_paste_replaces_the_selected_unicode_range_in_its_original_chat() {
    let (mut app, _) = app();
    let chat = ConversationId::Chat("19:paste".into());
    app.drafts.insert(
        chat.clone(),
        Draft {
            text: "A👋Z".into(),
            pending_pastes: 1,
            ..Default::default()
        },
    );
    app.open = Some(ConversationId::Chat("19:elsewhere".into()));
    app.handle(
        AccountEvent::Pasted {
            conversation: chat.clone(),
            range: (1, 2),
            text: "é".into(),
            image: Ok(None),
        },
        &egui::Context::default(),
    );
    assert_eq!(app.drafts[&chat].text, "AéZ");
    assert_eq!(app.drafts[&chat].paste_cursor, Some(2));
    assert_eq!(app.drafts[&chat].pending_pastes, 0);
}

#[test]
fn history_uses_the_visible_boundary_and_deduplicates_inflight_requests() {
    let (mut app, mut commands) = app();
    let chat = ConversationId::Chat("19:history".into());
    app.conversations.entry(chat.clone()).or_default().messages =
        vec![message("first", 200), message("last", 300)];
    let ctx = egui::Context::default();
    app.apply(Action::LoadOlder(chat.clone()), &ctx);
    app.apply(Action::LoadOlder(chat.clone()), &ctx);
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Account(AccountCommand::LoadOlder {
            before: Some(200),
            ..
        })
    ));
    assert!(commands.try_recv().is_err());
    app.handle(
        AccountEvent::Messages {
            conversation: chat.clone(),
            messages: vec![message("earlier", 100)],
            merge: Merge::Older,
            more: Some(true),
            cached: true,
        },
        &ctx,
    );
    assert!(app.conversations[&chat].restore_history_anchor);
    assert_eq!(app.conversations[&chat].messages[0].id, "earlier");
}

#[test]
fn relay_keys_are_saved_by_the_service_and_reconnect_every_account() {
    let (mut app, mut account_commands) = app();
    let (service, mut service_commands) = Backend::recording();
    app.service = service;
    app.settings.relay_url = Some("https://relay.example.com".into());
    let ctx = egui::Context::default();

    // Too short: refused here, nothing stored.
    app.apply(Action::SetRelayKey(Some("short".into())), &ctx);
    assert!(service_commands.try_recv().is_err());
    assert_eq!(app.toasts.last().unwrap().kind, ToastKind::Error);

    app.form_relay_key = " office-relay-key-0123 ".into();
    app.apply(Action::SetRelayKey(Some(app.form_relay_key.clone())), &ctx);
    match service_commands.try_recv().unwrap() {
        Command::Service(ServiceCommand::SaveRelayKey(Some(key))) => {
            assert_eq!(key.0, "office-relay-key-0123")
        }
        other => panic!("unexpected {other:?}"),
    }
    // Accounts reconnect only once the key is stored.
    assert!(account_commands.try_recv().is_err());
    app.handle_service(
        ServiceEvent::RelayKeySaved {
            set: true,
            ok: true,
        },
        &ctx,
    );
    assert!(app.settings.relay_key_set);
    assert!(app.form_relay_key.is_empty());
    assert!(matches!(
        account_commands.try_recv().unwrap(),
        Command::Account(AccountCommand::Relay(Some(url))) if url == "https://relay.example.com"
    ));

    // A failed store write changes nothing and says so.
    app.apply(Action::SetRelayKey(None), &ctx);
    assert!(matches!(
        service_commands.try_recv().unwrap(),
        Command::Service(ServiceCommand::SaveRelayKey(None))
    ));
    app.handle_service(
        ServiceEvent::RelayKeySaved {
            set: false,
            ok: false,
        },
        &ctx,
    );
    assert!(app.settings.relay_key_set);
    assert!(account_commands.try_recv().is_err());
    assert_eq!(app.toasts.last().unwrap().kind, ToastKind::Error);
}

#[test]
fn updates_wait_for_an_explicit_restart_and_do_not_discard_drafts() {
    let (mut app, _) = app();
    let (service, mut commands) = Backend::recording();
    app.service = service;
    let ctx = egui::Context::default();
    app.apply(Action::CheckUpdate, &ctx);
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Service(ServiceCommand::CheckUpdate { download: true })
    ));
    assert!(!app.quit_requested);
    let prepared = fastframe_update::Prepared::sample(
        fastframe_update::Installation {
            executable: PathBuf::from("/tmp/chaat"),
            kind: fastframe_update::Kind::Portable,
        },
        "0.1.1",
    );
    app.handle_service(
        ServiceEvent::Update(crate::updates::Event::Status(
            crate::updates::Status::Ready(Box::new(prepared)),
        )),
        &ctx,
    );
    app.drafts
        .entry(ConversationId::Chat("19:draft".into()))
        .or_default()
        .text = "Unsent".into();
    app.apply(Action::InstallUpdate, &ctx);
    assert!(commands.try_recv().is_err());
    assert!(!app.quit_requested);
    app.drafts.clear();
    app.apply(Action::InstallUpdate, &ctx);
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Service(ServiceCommand::InstallUpdate(_))
    ));
    assert!(
        !app.quit_requested,
        "only a successful helper handoff may quit"
    );
    app.handle_service(ServiceEvent::Update(crate::updates::Event::Quit), &ctx);
    assert!(app.quit_requested);
}

#[test]
fn changes_update_in_place_and_older_pages_prepend() {
    let mut conversation = Conversation::default();
    conversation.merge(vec![message("5", 5), message("6", 6)], Merge::Latest);
    let mut edited = message("6", 6);
    edited.edited = true;
    conversation.merge(vec![edited, message("7", 7)], Merge::Changes);
    conversation.merge(vec![message("1", 1)], Merge::Older);
    let ids: Vec<&str> = conversation
        .messages
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, ["1", "5", "6", "7"]);
    assert!(conversation.messages[2].edited);
}

#[test]
fn sending_is_optimistic_and_reconciled() {
    let (mut app, mut commands) = app();
    let chat = ConversationId::Chat("19:a".into());
    let ctx = egui::Context::default();
    app.apply(
        Action::Send {
            conversation: chat.clone(),
            text: "**hi**".into(),
            reply_to: None,
            mentions: Vec::new(),
            files: Vec::new(),
            images: Vec::new(),
        },
        &ctx,
    );
    let shown = &app.conversations[&chat].messages[0];
    assert_eq!(shown.delivery, Delivery::Sending);
    let Command::Account(AccountCommand::Send(crate::outbox::SendPayload {
        local_id, html, ..
    })) = next_command(&mut commands)
    else {
        panic!("send command expected");
    };
    assert_eq!(html, "<p><strong>hi</strong></p>");
    let mut confirmed = message("1700", crate::time::now());
    confirmed.author_id = Some("u-me".into());
    app.handle(
        AccountEvent::Sent {
            conversation: chat.clone(),
            local_id,
            message: Box::new(confirmed),
        },
        &ctx,
    );
    let messages = &app.conversations[&chat].messages;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "1700");
    assert_eq!(messages[0].delivery, Delivery::Sent);
}

#[test]
fn reactions_toggle_for_the_signed_in_person() {
    let mut reactions = Vec::new();
    toggle_reaction(&mut reactions, "👍", "me", true);
    toggle_reaction(&mut reactions, "👍", "ada", true);
    assert_eq!(reactions[0].users, ["me", "ada"]);
    toggle_reaction(&mut reactions, "👍", "me", false);
    toggle_reaction(&mut reactions, "👍", "ada", false);
    assert!(reactions.is_empty());
}

#[test]
fn opening_a_chat_fetches_it_and_marks_it_read() {
    let (mut app, mut commands) = app();
    app.chats = vec![Chat {
        id: "19:a".into(),
        preview: Some(crate::model::Preview {
            message_id: "1".into(),
            author_id: Some("u-ada".into()),
            at: 10,
            ..Default::default()
        }),
        last_read: Some(1),
        ..Default::default()
    }];
    let ctx = egui::Context::default();
    app.apply(
        Action::OpenConversation(ConversationId::Chat("19:a".into())),
        &ctx,
    );
    let mut seen = Vec::new();
    while let Ok(command) = commands.try_recv() {
        seen.push(format!("{command:?}"));
    }
    assert!(seen.iter().any(|c| {
        c.starts_with("Account(Fetch { resource: Messages(Chat(\"19:a\")), trigger: Opened")
    }));
    assert!(seen.iter().any(|c| c == "Account(MarkRead(\"19:a\"))"));
    assert!(!app.chats[0].is_unread("u-me"));
}

#[test]
fn losing_the_connection_is_a_quiet_marker_not_a_toast() {
    let (mut app, _commands) = app();
    let ctx = egui::Context::default();
    app.handle(AccountEvent::Reachable(false), &ctx);
    assert!(app.unreachable);
    assert!(app.toasts.is_empty());
    app.handle(AccountEvent::Reachable(true), &ctx);
    assert!(!app.unreachable);
}

#[test]
fn messages_arriving_in_the_background_stay_unread() {
    let (mut app, mut commands) = app();
    let chat = ConversationId::Chat("19:a".into());
    app.chats = vec![Chat {
        id: "19:a".into(),
        preview: Some(crate::model::Preview {
            message_id: "1".into(),
            author_id: Some("u-ada".into()),
            at: 10,
            ..Default::default()
        }),
        last_read: Some(20),
        ..Default::default()
    }];
    app.open = Some(chat.clone());
    app.window_focused = false;
    let ctx = egui::Context::default();
    let incoming = |id: &str, at| Message {
        author_id: Some("u-ada".into()),
        ..message(id, at)
    };
    app.handle(
        AccountEvent::Messages {
            conversation: chat.clone(),
            messages: vec![incoming("2", 300)],
            merge: Merge::Latest,
            more: None,
            cached: false,
        },
        &ctx,
    );
    let mut seen = Vec::new();
    while let Ok(command) = commands.try_recv() {
        seen.push(format!("{command:?}"));
    }
    assert!(!seen.iter().any(|c| c.starts_with("Account(MarkRead")));
    assert!(app.chats[0].is_unread("u-me"));

    app.window_focused = true;
    app.handle(
        AccountEvent::Messages {
            conversation: chat,
            messages: vec![incoming("3", 400)],
            merge: Merge::Latest,
            more: None,
            cached: false,
        },
        &ctx,
    );
    let mut seen = Vec::new();
    while let Ok(command) = commands.try_recv() {
        seen.push(format!("{command:?}"));
    }
    assert!(seen.iter().any(|c| c == "Account(MarkRead(\"19:a\"))"));
    assert!(!app.chats[0].is_unread("u-me"));
}

#[test]
fn only_teams_links_are_joined() {
    let (mut app, _commands) = app();
    let ctx = egui::Context::default();
    app.apply(Action::JoinMeeting("https://evil.example/x".into()), &ctx);
    assert_eq!(app.toasts.len(), 1);
}

#[test]
fn mentions_and_files_travel_with_a_send() {
    let (mut app, mut commands) = app();
    let chat = ConversationId::Chat("19:a".into());
    app.chats = vec![Chat {
        id: "19:a".into(),
        members: vec![
            Member {
                tenant_id: None,
                user_id: "u-me".into(),
                display_name: "Me".into(),
                email: Some("me@contoso.com".into()),
            },
            Member {
                tenant_id: None,
                user_id: "u-ada".into(),
                display_name: "Ada".into(),
                email: Some("ada@contoso.com".into()),
            },
        ],
        ..Default::default()
    }];
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("plan.pdf");
    std::fs::write(&file, b"%PDF").unwrap();
    let ctx = egui::Context::default();
    app.apply(
        Action::Send {
            conversation: chat.clone(),
            text: "@Ada see the plan".into(),
            reply_to: None,
            mentions: vec![MentionRef {
                user_id: "u-ada".into(),
                name: "Ada".into(),
            }],
            files: vec![file.clone()],
            images: Vec::new(),
        },
        &ctx,
    );
    let Command::Account(AccountCommand::Send(crate::outbox::SendPayload {
        html,
        mentions,
        files,
        recipients,
        ..
    })) = next_command(&mut commands)
    else {
        panic!("send command expected");
    };
    assert_eq!(html, "<p><at id=\"0\">Ada</at> see the plan</p>");
    assert_eq!(mentions.len(), 1);
    assert_eq!(files, vec![file]);
    assert_eq!(recipients, vec!["ada@contoso.com".to_owned()]);
    let shown = &app.conversations[&chat].messages[0];
    assert_eq!(shown.attachments[0].name.as_deref(), Some("plan.pdf"));
}

#[test]
fn out_of_office_asks_for_its_scope_then_fetches_and_saves() {
    use crate::model::{AutoReplies, OutOfOffice};
    let (mut app, mut commands) = app();
    app.granted = "User.Read Chat.ReadWrite ChatMember.ReadWrite".into();
    let ctx = egui::Context::default();
    app.apply(Action::OpenOutOfOffice, &ctx);
    assert_eq!(app.settings_page, Some(SettingsPage::OutOfOffice));
    assert_eq!(
        app.overlay,
        Some(Overlay::Consent(crate::auth::MAILBOX_SETTINGS_SCOPE))
    );
    assert!(commands.try_recv().is_err(), "no fetch without the scope");

    // Consent arrives while the page is open: look once.
    app.handle(
        AccountEvent::Granted("User.Read MailboxSettings.ReadWrite".into()),
        &ctx,
    );
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::Fetch {
            resource: Resource::OutOfOffice,
            trigger: Trigger::Opened
        }))
    ));
    // A later session restore with the scope already held looks no more.
    app.handle(
        AccountEvent::Granted("User.Read MailboxSettings.ReadWrite".into()),
        &ctx,
    );
    assert!(commands.try_recv().is_err());
    let away = OutOfOffice {
        replies: AutoReplies::On,
        internal: "Away".into(),
        ..Default::default()
    };
    app.handle(AccountEvent::OutOfOffice(away.clone()), &ctx);
    assert_eq!(app.form_out_of_office.internal, "Away");

    // Saving shows as under way until Graph answers; a failure keeps
    // the edit so the person can try again.
    app.form_out_of_office.replies = AutoReplies::Off;
    let off = app.form_out_of_office.value().unwrap();
    app.apply(Action::SetOutOfOffice(off.clone()), &ctx);
    assert!(app.out_of_office_saving);
    assert!(
        matches!(commands.try_recv(), Ok(Command::Account(AccountCommand::SetOutOfOffice(v))) if v == off)
    );
    app.apply(Action::SetOutOfOffice(off.clone()), &ctx);
    assert!(commands.try_recv().is_err(), "one save at a time");
    app.handle(AccountEvent::OutOfOfficeNotSaved, &ctx);
    assert!(!app.out_of_office_saving);
    assert_eq!(app.form_out_of_office.replies, AutoReplies::Off);
    assert!(app.form_out_of_office.changed_from(&away));

    // A fetch while the person edits leaves the edit alone.
    app.handle(AccountEvent::OutOfOffice(away.clone()), &ctx);
    assert_eq!(app.form_out_of_office.replies, AutoReplies::Off);
    app.handle(AccountEvent::OutOfOffice(off.clone()), &ctx);
    assert_eq!(app.out_of_office, Some(off));
    assert_eq!(app.form_out_of_office.replies, AutoReplies::Off);

    app.handle(AccountEvent::OutOfOfficeUnavailable, &ctx);
    assert!(app.no_mailbox);
}

#[test]
fn features_needing_new_permissions_ask_first() {
    let (mut app, mut commands) = app();
    app.granted = "User.Read Chat.ReadWrite ChatMember.ReadWrite".into();
    let ctx = egui::Context::default();
    app.apply(Action::OpenNewChat, &ctx);
    assert_eq!(
        app.overlay,
        Some(Overlay::Consent(crate::auth::CHAT_CREATE_SCOPE))
    );
    app.apply(Action::SetAvailability(Some(Availability::Busy)), &ctx);
    assert_eq!(
        app.overlay,
        Some(Overlay::Consent(crate::auth::PRESENCE_WRITE_SCOPE))
    );
    app.apply(
        Action::RequestScope(crate::auth::PRESENCE_WRITE_SCOPE),
        &ctx,
    );
    let Ok(Command::Account(AccountCommand::SignIn { extra_scopes, .. })) = commands.try_recv()
    else {
        panic!("consent sign-in expected");
    };
    assert_eq!(extra_scopes, vec!["Presence.ReadWrite".to_owned()]);
    app.granted.push_str(" Presence.ReadWrite");
    app.apply(Action::SetAvailability(Some(Availability::Busy)), &ctx);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::SetAvailability(Some(
            Availability::Busy
        ))))
    ));
    assert_eq!(
        app.my_presence.as_ref().unwrap().availability,
        Availability::Busy
    );
}

#[test]
fn hidden_accounts_update_without_touching_the_screen() {
    let (mut app, _commands) = app();
    let (second, _second_commands) = Backend::recording();
    app.accounts.push(Account::new(second));
    app.accounts[1].auth = AuthStatus::SignedIn {
        account: "u-me2.t2".into(),
        name: "Me Too".into(),
        username: "me@fabrikam.com".into(),
    };
    app.accounts[1].open = Some(ConversationId::Chat("19:b".into()));
    let ctx = egui::Context::default();
    // An event for account 1 while account 0 shows.
    app.active = 1;
    app.hidden = true;
    app.handle(
        AccountEvent::Chats {
            chats: vec![Chat {
                id: "19:b".into(),
                ..Default::default()
            }],
            cached: false,
        },
        &ctx,
    );
    app.hidden = false;
    app.active = 0;
    assert!(app.chats.is_empty());
    assert_eq!(app.accounts[1].chats.len(), 1);
    app.apply(Action::SwitchAccount(1), &ctx);
    assert_eq!(app.active, 1);
    assert_eq!(app.chats[0].id, "19:b");
    assert_eq!(app.label(), "Me Too");
}

#[test]
fn signing_in_twice_keeps_one_slot() {
    let (mut app, _commands) = app();
    app.accounts[0].id = Some("u-me.t1".into());
    let (second, _second_commands) = Backend::recording();
    app.accounts.push(Account::new(second));
    app.active = 1;
    let ctx = egui::Context::default();
    app.handle(
        AccountEvent::Auth(AuthStatus::SignedIn {
            account: "u-me.t1".into(),
            name: "Me".into(),
            username: "me@contoso.com".into(),
        }),
        &ctx,
    );
    app.retire_duplicates();
    app.apply_actions(&ctx);
    assert_eq!(app.accounts.len(), 1);
    assert_eq!(app.active, 0);
}

#[test]
fn unread_only_lists_unread_chats_and_keeps_one_read_while_open() {
    let (mut app, _commands) = app();
    let chat = |id: &str, last_read: i64| Chat {
        id: id.into(),
        preview: Some(crate::model::Preview {
            message_id: "1".into(),
            author_id: Some("u-ada".into()),
            at: 10,
            ..Default::default()
        }),
        last_read: Some(last_read),
        ..Default::default()
    };
    app.chats = vec![chat("19:unread", 1), chat("19:read", 10)];
    let me = app.me().to_owned();
    let shown = |app: &App| -> Vec<String> {
        app.chats
            .iter()
            .filter(|c| chat_filter::listed(app, c, &me))
            .map(|c| c.id.clone())
            .collect()
    };
    assert_eq!(shown(&app).len(), 2);
    app.unread_only = true;
    assert_eq!(shown(&app), vec!["19:unread".to_owned()]);
    let ctx = egui::Context::default();
    // An open chat that was already read does not show.
    app.apply(
        Action::OpenConversation(ConversationId::Chat("19:read".into())),
        &ctx,
    );
    assert_eq!(shown(&app), vec!["19:unread".to_owned()]);
    // Opening an unread chat reads it, and it stays while open.
    app.apply(
        Action::OpenConversation(ConversationId::Chat("19:unread".into())),
        &ctx,
    );
    assert!(!app.chats[0].is_unread(&me));
    assert_eq!(shown(&app), vec!["19:unread".to_owned()]);
    // Once another chat opens, it goes.
    app.apply(
        Action::OpenConversation(ConversationId::Chat("19:read".into())),
        &ctx,
    );
    assert!(shown(&app).is_empty());
}

#[test]
fn an_expired_account_signs_in_again_over_its_cached_chats() {
    let (mut app, mut commands) = app();
    app.chats = vec![Chat {
        id: "19:a".into(),
        ..Default::default()
    }];
    let expired = AuthStatus::Expired {
        username: "me@contoso.com".into(),
        reason: "Sign in again to get new messages.".into(),
    };
    app.on_auth(expired.clone());
    assert!(app.shows_workspace());
    let ctx = egui::Context::default();

    // Cancelling goes back to the cached workspace.
    app.apply(Action::SignIn, &ctx);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::SignIn { .. }))
    ));
    app.on_auth(AuthStatus::WaitingForBrowser { url: "u".into() });
    assert!(app.shows_workspace());
    app.apply(Action::CancelSignIn, &ctx);
    assert_eq!(app.auth, expired);
    assert!(app.reauth.is_none());

    // A failure keeps the workspace and says why.
    app.apply(Action::SignIn, &ctx);
    app.on_auth(AuthStatus::Failed("declined".into()));
    assert!(matches!(
        &app.auth,
        AuthStatus::Expired { username, reason }
            if username == "me@contoso.com" && reason == "declined"
    ));
    assert!(app.shows_workspace());

    app.apply(Action::SignIn, &ctx);
    app.on_auth(AuthStatus::SignedIn {
        account: "u-me.t1".into(),
        name: "Me".into(),
        username: "me@contoso.com".into(),
    });
    assert!(app.reauth.is_none());
    assert!(app.is_signed_in());
}

fn drain(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
    std::iter::from_fn(|| commands.try_recv().ok()).collect()
}

fn entry(local_id: &str, text: &str, created: i64) -> crate::outbox::OutboxEntry {
    crate::outbox::OutboxEntry {
        payload: crate::outbox::SendPayload {
            local_id: local_id.into(),
            conversation: ConversationId::Chat("19:a".into()),
            html: html::compose(text),
            reply_to: None,
            mentions: Vec::new(),
            files: Vec::new(),
            images: Vec::new(),
            recipients: Vec::new(),
        },
        created,
        text: text.into(),
    }
}

#[test]
fn draft_saved_on_change() {
    let (mut app, mut commands) = app();
    let chat = ConversationId::Chat("19:a".into());
    app.sync_drafts();
    assert!(drain(&mut commands).is_empty(), "no drafts, nothing saved");
    app.drafts.entry(chat.clone()).or_default().text = "hel".into();
    app.sync_drafts();
    app.sync_drafts();
    let saved = drain(&mut commands);
    assert_eq!(saved.len(), 1, "one save, none when unchanged");
    assert!(
        matches!(&saved[0], Command::Account(AccountCommand::SaveDraft { conversation, draft: Some(d) })
        if conversation == &chat && d.text == "hel")
    );
    app.drafts.entry(chat.clone()).or_default().text = "hello".into();
    app.sync_drafts();
    assert_eq!(drain(&mut commands).len(), 1);
    // Files and images are not saved: a draft holding only them is empty.
    app.drafts.get_mut(&chat).unwrap().text.clear();
    app.drafts
        .get_mut(&chat)
        .unwrap()
        .files
        .push("x.txt".into());
    app.sync_drafts();
    assert!(matches!(
        drain(&mut commands).as_slice(),
        [Command::Account(AccountCommand::SaveDraft {
            draft: None,
            ..
        })]
    ));
    // An edit in progress does not touch the saved draft.
    app.drafts.get_mut(&chat).unwrap().editing = Some("m1".into());
    app.drafts.get_mut(&chat).unwrap().text = "edited".into();
    app.sync_drafts();
    assert!(drain(&mut commands).is_empty());
}

#[test]
fn cleared_on_send() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    let chat = ConversationId::Chat("19:a".into());
    app.drafts.entry(chat.clone()).or_default().text = "hi".into();
    app.sync_drafts();
    drain(&mut commands);
    app.actions.push(Action::Send {
        conversation: chat.clone(),
        text: "hi".into(),
        reply_to: None,
        mentions: Vec::new(),
        files: Vec::new(),
        images: Vec::new(),
    });
    app.apply_actions(&ctx);
    let sent = drain(&mut commands);
    assert!(matches!(
        sent[0],
        Command::Account(AccountCommand::SaveOutbox(_))
    ));
    assert!(matches!(
        sent[1],
        Command::Account(AccountCommand::Send(crate::outbox::SendPayload { .. }))
    ));
    assert!(matches!(
        sent.last(),
        Some(Command::Account(AccountCommand::SaveDraft {
            draft: None,
            ..
        }))
    ));
}

#[test]
fn restart_restores() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    let chat = ConversationId::Chat("19:a".into());
    let now = crate::time::now();
    let draft = crate::outbox::StoredDraft {
        text: "see you\nlater".into(),
        reply_to: Some("m1".into()),
        mentions: Vec::new(),
    };
    app.handle(
        AccountEvent::Restored {
            drafts: vec![(chat.clone(), draft.clone())],
            outbox: vec![
                entry("local-7", "young", now - 1_000),
                entry("local-3", "old", now - 700_000),
            ],
        },
        &ctx,
    );
    assert_eq!(app.drafts[&chat].text, "see you\nlater");
    assert_eq!(app.drafts[&chat].reply_to.as_deref(), Some("m1"));
    assert_eq!(app.draft_preview("19:a").as_deref(), Some("see you later"));
    assert_eq!(app.draft_preview("19:none"), None);
    let shown = &app.conversations[&chat].messages;
    let state = |id: &str| shown.iter().find(|m| m.id == id).unwrap().delivery;
    assert_eq!(state("local-7"), Delivery::Sending);
    assert_eq!(state("local-3"), Delivery::Failed);
    let sent = drain(&mut commands);
    let resent: Vec<&str> = sent
        .iter()
        .filter_map(|c| match c {
            Command::Account(AccountCommand::Resend(e)) => Some(e.payload.local_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(resent, ["local-7"], "only the young entry resends");
    assert!(
        sent.iter()
            .any(|c| matches!(c, Command::Account(AccountCommand::CachedMessages(_))))
    );
    // The restored draft is already saved: nothing is written back.
    app.sync_drafts();
    assert!(drain(&mut commands).is_empty());
    // Retry works for the failed one, and new ids do not collide.
    app.apply(
        Action::RetrySend {
            conversation: chat.clone(),
            local_id: "local-3".into(),
        },
        &ctx,
    );
    assert!(matches!(
        drain(&mut commands).as_slice(),
        [Command::Account(AccountCommand::Send(crate::outbox::SendPayload { local_id, .. }))] if local_id == "local-3"
    ));
    app.apply(
        Action::Send {
            conversation: chat,
            text: "new".into(),
            reply_to: None,
            mentions: Vec::new(),
            files: Vec::new(),
            images: Vec::new(),
        },
        &ctx,
    );
    let Command::Account(AccountCommand::Send(crate::outbox::SendPayload { local_id, .. })) =
        next_command(&mut commands)
    else {
        panic!("send expected")
    };
    assert_eq!(local_id, "local-8");
}

#[test]
fn a_draft_typed_before_restore_is_kept() {
    let (mut app, _) = app();
    let chat = ConversationId::Chat("19:a".into());
    app.drafts.entry(chat.clone()).or_default().text = "typing".into();
    app.handle(
        AccountEvent::Restored {
            drafts: vec![(
                chat.clone(),
                crate::outbox::StoredDraft {
                    text: "old".into(),
                    ..Default::default()
                },
            )],
            outbox: Vec::new(),
        },
        &egui::Context::default(),
    );
    assert_eq!(app.drafts[&chat].text, "typing");
}

#[test]
fn delete_failed() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    let chat = ConversationId::Chat("19:a".into());
    app.handle(
        AccountEvent::Restored {
            drafts: Vec::new(),
            outbox: vec![entry("local-3", "old", 5)],
        },
        &ctx,
    );
    drain(&mut commands);
    assert_eq!(app.conversations[&chat].messages.len(), 1);
    app.apply(
        Action::DeleteFailed {
            conversation: chat.clone(),
            local_id: "local-3".into(),
        },
        &ctx,
    );
    assert!(app.conversations[&chat].messages.is_empty());
    assert!(app.pending_sends.is_empty());
    assert!(matches!(
        drain(&mut commands).as_slice(),
        [Command::Account(AccountCommand::DropOutbox(id))] if id == "local-3"
    ));
}

#[test]
fn update_gate() {
    let (mut app, _) = app();
    let (service, mut commands) = Backend::recording();
    app.service = service;
    let ctx = egui::Context::default();
    let prepared = fastframe_update::Prepared::sample(
        fastframe_update::Installation {
            executable: PathBuf::from("/tmp/chaat"),
            kind: fastframe_update::Kind::Portable,
        },
        "0.1.1",
    );
    app.handle_service(
        ServiceEvent::Update(crate::updates::Event::Status(
            crate::updates::Status::Ready(Box::new(prepared)),
        )),
        &ctx,
    );
    let chat = ConversationId::Chat("19:draft".into());
    app.drafts.entry(chat.clone()).or_default().text = "Unsent".into();
    app.apply(Action::InstallUpdate, &ctx);
    assert!(commands.try_recv().is_err(), "unsaved text blocks");
    app.sync_drafts();
    app.pending_sends.insert(
        "local-1".into(),
        PendingSend {
            html: String::new(),
            reply_to: None,
            mentions: Vec::new(),
            files: Vec::new(),
            images: Vec::new(),
            recipients: Vec::new(),
        },
    );
    app.apply(Action::InstallUpdate, &ctx);
    assert!(commands.try_recv().is_err(), "pending sends still block");
    app.pending_sends.clear();
    app.apply(Action::InstallUpdate, &ctx);
    assert!(
        matches!(
            commands.try_recv(),
            Ok(Command::Service(ServiceCommand::InstallUpdate(_)))
        ),
        "a saved text draft no longer blocks"
    );
}

fn hit(chat: &str, id: &str, at: i64) -> SearchHit {
    SearchHit {
        conversation: ConversationId::Chat(chat.into()),
        message_id: id.into(),
        author: "Ada".into(),
        snippet: "lunch".into(),
        at,
    }
}

fn finding(query: &str) -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
    let (mut app, mut commands) = app();
    app.open = Some(ConversationId::Chat("19:a".into()));
    app.apply(Action::Find(Some(query.into())), &egui::Context::default());
    drain(&mut commands);
    (app, commands)
}

#[test]
fn find_asks_the_backend_for_two_characters_or_more() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    let chat = ConversationId::Chat("19:a".into());
    app.apply(Action::Find(Some("lunch".into())), &ctx);
    assert!(commands.try_recv().is_err(), "nothing open, nothing sent");
    app.open = Some(chat.clone());
    app.apply(Action::Find(Some("l".into())), &ctx);
    assert!(commands.try_recv().is_err(), "one character sends nothing");
    app.apply(Action::Find(Some("lu".into())), &ctx);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::SearchIn { conversation, query, remote: false })) if conversation == chat && query == "lu"
    ));
    app.apply(Action::Find(Some("lu".into())), &ctx);
    assert!(
        commands.try_recv().is_err(),
        "an unchanged query is not asked twice"
    );
    app.apply(Action::Find(None), &ctx);
    assert!(app.find.is_none());
}

#[test]
fn find_waits_for_a_pause_before_microsoft_search() {
    let (mut app, mut commands) = app();
    let ctx = egui::Context::default();
    let chat = ConversationId::Chat("19:a".into());
    app.open = Some(chat.clone());
    let mut typed = String::new();
    for letter in "lunch".chars() {
        typed.push(letter);
        app.apply(Action::Find(Some(typed.clone())), &ctx);
        app.send_due_find_search(Instant::now(), &ctx);
    }
    let mut sent = Vec::new();
    while let Ok(command) = commands.try_recv() {
        sent.push(command);
    }
    assert_eq!(sent.len(), 4, "local searches for lu, lun, lunc, lunch");
    assert!(
        sent.iter().all(|c| matches!(
            c,
            Command::Account(AccountCommand::SearchIn { remote: false, .. })
        )),
        "typing sends local searches only"
    );
    // Not yet due: still nothing.
    app.send_due_find_search(Instant::now(), &ctx);
    assert!(commands.try_recv().is_err());
    // Typing paused for 500 ms: one Microsoft Search, for the last query.
    let later = Instant::now() + Duration::from_millis(500);
    app.send_due_find_search(later, &ctx);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::SearchIn { conversation, query, remote: true }))
            if conversation == chat && query == "lunch"
    ));
    app.send_due_find_search(later, &ctx);
    assert!(commands.try_recv().is_err(), "never sent twice");
}

#[test]
fn find_enter_sends_microsoft_search_at_once() {
    let (mut app, mut commands) = finding("lunch");
    let ctx = egui::Context::default();
    app.apply(Action::FindNext(1), &ctx);
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::Account(AccountCommand::SearchIn { query, remote: true, .. })) if query == "lunch"
    ));
    app.apply(Action::FindNext(1), &ctx);
    app.send_due_find_search(Instant::now() + Duration::from_secs(1), &ctx);
    assert!(commands.try_recv().is_err(), "once per query");
}

#[test]
fn find_closed_or_moved_on_sends_no_microsoft_search() {
    let later = Instant::now() + Duration::from_secs(1);
    let ctx = egui::Context::default();
    let (mut app, mut commands) = finding("lunch");
    app.apply(Action::Find(None), &ctx);
    app.send_due_find_search(later, &ctx);
    assert!(commands.try_recv().is_err(), "closed before 500 ms");
    // Enter on a closed bar finds nothing to send either.
    app.apply(Action::FindNext(1), &ctx);
    assert!(commands.try_recv().is_err());

    let (mut app, mut commands) = finding("lunch");
    app.open = Some(ConversationId::Chat("19:b".into()));
    app.send_due_find_search(later, &ctx);
    assert!(commands.try_recv().is_err(), "another conversation");

    let (mut app, mut commands) = finding("lunch");
    app.apply(Action::Find(Some("lu".into())), &ctx);
    drain(&mut commands);
    app.send_due_find_search(later, &ctx);
    assert!(commands.try_recv().is_err(), "two characters stay local");
}

#[test]
fn stale_hits_ignored() {
    let (mut app, _) = finding("lunch");
    let ctx = egui::Context::default();
    let chat = ConversationId::Chat("19:a".into());
    app.handle(
        AccountEvent::ConversationHits {
            conversation: chat.clone(),
            query: "lun".into(),
            hits: vec![hit("19:a", "old", 1)],
        },
        &ctx,
    );
    app.handle(
        AccountEvent::ConversationHits {
            conversation: ConversationId::Chat("19:b".into()),
            query: "lunch".into(),
            hits: vec![hit("19:b", "other", 1)],
        },
        &ctx,
    );
    assert!(app.find.as_ref().unwrap().hits.is_empty());
    // Hits for another chat inside the right event are dropped too.
    app.handle(
        AccountEvent::ConversationHits {
            conversation: chat,
            query: "lunch".into(),
            hits: vec![hit("19:a", "a1", 1), hit("19:b", "stray", 2)],
        },
        &ctx,
    );
    let ids: Vec<_> = app
        .find
        .as_ref()
        .unwrap()
        .hits
        .iter()
        .map(|h| h.message_id.as_str())
        .collect();
    assert_eq!(ids, ["a1"]);
    // A query typed since the search leaves its hits behind.
    app.apply(Action::Find(Some("lunches".into())), &ctx);
    assert!(app.find.as_ref().unwrap().hits.is_empty());
}

#[test]
fn find_next_wraps() {
    let (mut app, _) = finding("lunch");
    let ctx = egui::Context::default();
    app.handle(
        AccountEvent::ConversationHits {
            conversation: ConversationId::Chat("19:a".into()),
            query: "lunch".into(),
            hits: vec![
                hit("19:a", "a", 1),
                hit("19:a", "c", 3),
                hit("19:a", "b", 2),
            ],
        },
        &ctx,
    );
    // Newest first; the first hits bring the newest into view.
    assert_eq!(app.jump_to.as_deref(), Some("c"));
    assert_eq!(app.find.as_ref().unwrap().current, 0);
    app.apply(Action::FindNext(1), &ctx);
    assert_eq!(app.find.as_ref().unwrap().current, 1);
    app.apply(Action::FindNext(1), &ctx);
    app.apply(Action::FindNext(1), &ctx);
    assert_eq!(app.find.as_ref().unwrap().current, 0, "wraps forward");
    app.apply(Action::FindNext(-1), &ctx);
    assert_eq!(app.find.as_ref().unwrap().current, 2, "wraps back");
}

#[test]
fn jump_sets_jump_to() {
    let (mut app, _) = finding("lunch");
    let ctx = egui::Context::default();
    app.handle(
        AccountEvent::ConversationHits {
            conversation: ConversationId::Chat("19:a".into()),
            query: "lunch".into(),
            hits: vec![hit("19:a", "new", 2), hit("19:a", "old", 1)],
        },
        &ctx,
    );
    app.jump_to = None;
    app.apply(Action::FindNext(1), &ctx);
    assert_eq!(app.jump_to.as_deref(), Some("old"));
    // A later batch (Microsoft Search) keeps the hit being shown.
    app.handle(
        AccountEvent::ConversationHits {
            conversation: ConversationId::Chat("19:a".into()),
            query: "lunch".into(),
            hits: vec![
                hit("19:a", "newest", 3),
                hit("19:a", "new", 2),
                hit("19:a", "old", 1),
            ],
        },
        &ctx,
    );
    let find = app.find.as_ref().unwrap();
    assert_eq!(find.hits[find.current].message_id, "old");
    app.scroll_to_end = 0;
    app.apply(Action::JumpToLatest, &ctx);
    assert!(app.scroll_to_end > 0);
}

#[test]
fn opening_another_conversation_closes_find() {
    let (mut app, _) = finding("lunch");
    app.apply(
        Action::OpenConversation(ConversationId::Chat("19:b".into())),
        &egui::Context::default(),
    );
    assert!(app.find.is_none());
}
