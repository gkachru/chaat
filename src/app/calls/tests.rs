use super::*;
use crate::app::tests::*;
use crate::calling::session::GRACE;

fn demo() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(
        crate::paths::AppDirs::under(dir.path()),
        crate::settings::Settings::default(),
        crate::backend::Waker::default(),
        true,
    );
    (app, dir)
}

fn alert(id: &str, caller: &str, object_id: &str) -> crate::calling::RingAlert {
    crate::calling::RingAlert {
        id: id.into(),
        caller: caller.into(),
        caller_object_id: Some(object_id.into()),
        video: false,
        group: false,
        thread_id: None,
        account: None,
        username: None,
        ring_timeout: None,
    }
}

#[test]
fn a_ring_raises_the_window_once_per_call_and_at_most_every_ten_seconds() {
    let (mut app, _dir) = demo();
    app.wants_show = false;
    app.on_incoming_call(alert("c1", "A", "u-ada"));
    assert!(app.wants_show);
    // The same call again: no second raise.
    app.wants_show = false;
    app.on_incoming_call(alert("c1", "A", "u-ada"));
    assert!(!app.wants_show);
    // Another call right after: the card shows, the window stays put.
    app.on_incoming_call(alert("c2", "B", "u-ada"));
    assert!(!app.wants_show);
    assert_eq!(app.rings.len(), 2);

    // The gap and the memory, directly.
    let mut raises = RingRaises::default();
    let t0 = std::time::Instant::now();
    assert!(raises.allow("a", t0));
    assert!(!raises.allow("b", t0 + std::time::Duration::from_secs(9)));
    assert!(raises.allow("c", t0 + std::time::Duration::from_secs(10)));
    assert!(!raises.allow("c", t0 + std::time::Duration::from_secs(60)));
    // "b" never raised, and it never will.
    assert!(!raises.allow("b", t0 + std::time::Duration::from_secs(60)));
    // Only the last 16 calls are remembered.
    for n in 0..16 {
        raises.allow(&format!("x{n}"), t0);
    }
    assert!(raises.seen.len() <= 16);
    assert!(raises.allow("a", t0 + std::time::Duration::from_secs(600)));
}

#[test]
fn the_card_names_the_resolved_member_not_the_plugin() {
    let (mut app, _dir) = demo();
    app.on_incoming_call(alert("c1", "Totally Not Ada", "u-ada"));
    let shown = &app.rings[0];
    assert!(shown.member.is_some());
    assert_eq!(shown.caller, "Ada Lovelace");
}

#[test]
fn an_unresolved_caller_is_marked_as_from_the_plugin() {
    let (mut app, _dir) = demo();
    app.on_incoming_call(alert("c1", "Someone New", "u-nobody"));
    let shown = &app.rings[0];
    assert_eq!(shown.caller, "Someone New");
    // The card marks a name without a member as the plugin's.
    assert!(shown.member.is_none());
}

#[test]
fn a_thread_without_the_caller_is_not_opened() {
    let (mut app, _dir) = demo();
    let without = app
        .chats
        .iter()
        .find(|c| c.members.iter().all(|m| m.user_id != "u-ada"))
        .map(|c| c.id.clone())
        .expect("a demo chat without Ada");
    let with = app
        .chats
        .iter()
        .find(|c| c.kind != ChatKind::OneOnOne && c.members.iter().any(|m| m.user_id == "u-ada"))
        .map(|c| c.id.clone())
        .expect("a demo group chat with Ada");
    let mut ring = alert("c1", "Ada", "u-ada");
    ring.thread_id = Some(without.clone());
    app.on_incoming_call(ring);
    assert_ne!(
        app.rings[0].chat,
        Some(ConversationId::Chat(without.clone()))
    );
    // Nobody resolved: no thread at all.
    let mut ring = alert("c2", "Ada", "u-nobody");
    ring.thread_id = Some(with.clone());
    app.on_incoming_call(ring);
    assert_eq!(app.rings[1].chat, None);
    // The caller is in it: kept.
    let mut ring = alert("c3", "Ada", "u-ada");
    ring.thread_id = Some(with.clone());
    app.on_incoming_call(ring);
    assert_eq!(app.rings[2].chat, Some(ConversationId::Chat(with)));
}

const UNKNOWN: &str =
    "https://teams.microsoft.com/l/meetup-join/19%3ameeting_elsewhere%40thread.v2/0";

#[test]
fn presence_ends_a_call_after_the_grace_period() {
    let (mut app, _dir) = demo();
    let ctx = egui::Context::default();
    app.join_meeting(UNKNOWN.into(), &ctx);
    app.pretend_docked();
    // The demo treats the call as live at once.
    app.observe_call(Some("Available"));
    let Some(call) = &mut app.call else {
        panic!("a call")
    };
    let CallState::Leaving { since, .. } = call.state else {
        panic!("leaving, not {:?}", call.state)
    };
    // A minute later presence still says the call is over.
    call.state = CallState::Leaving {
        since,
        left: crate::time::now() - GRACE - 1,
    };
    app.tick_call(&ctx);
    assert!(app.call.as_ref().is_some_and(CallSession::is_ended));
    assert!(
        app.docked.is_none(),
        "the window is restored when the call ends"
    );
}

#[test]
fn calls_elsewhere_show_but_calendar_meetings_do_not() {
    let (mut app, _dir) = demo();
    app.demo = false;
    app.observe_call(Some("InAMeeting"));
    assert!(app.call.is_none());
    app.observe_call(Some("InACall"));
    assert_eq!(app.call.as_ref().unwrap().target, CallTarget::Elsewhere);
}

#[test]
fn a_dismissed_call_bar_stays_closed_until_the_call_ends() {
    let (mut app, _dir) = demo();
    app.demo = false;
    let ctx = egui::Context::default();
    app.observe_call(Some("InACall"));
    assert!(app.call.is_some());
    app.apply(Action::CallDone, &ctx);
    assert!(app.call.is_none());
    // Presence still says the call goes on: the bar stays closed.
    app.observe_call(Some("InACall"));
    assert!(app.call.is_none());
    // The call ended; the next one shows again.
    app.observe_call(Some("Available"));
    app.observe_call(Some("InACall"));
    assert!(app.call.is_some());
}

#[test]
fn a_meeting_chat_found_later_is_shown_beside_the_call() {
    let (mut app, _dir) = demo();
    let ctx = egui::Context::default();
    app.join_meeting(UNKNOWN.into(), &ctx);
    assert_eq!(app.call.as_ref().unwrap().lookup, ChatLookup::NotFound);
    assert_eq!(app.call.as_ref().unwrap().title(), "Meeting");
    let chat = Chat {
        id: "19:meeting_elsewhere@thread.v2".into(),
        kind: ChatKind::Meeting,
        join_url: Some(UNKNOWN.into()),
        ..Default::default()
    };
    // Another meeting's answer is kept but not attached.
    app.on_meeting_chat(
        "https://teams.microsoft.com/l/meetup-join/other/0".into(),
        None,
    );
    assert_eq!(app.call.as_ref().unwrap().lookup, ChatLookup::NotFound);
    app.on_meeting_chat(UNKNOWN.into(), Some(Box::new(chat)));
    let call = app.call.as_ref().unwrap();
    assert_eq!(call.lookup, ChatLookup::Found);
    let id = ConversationId::Chat("19:meeting_elsewhere@thread.v2".into());
    assert_eq!(call.chat.as_ref(), Some(&id));
    assert_eq!(app.open.as_ref(), Some(&id));
    assert!(
        app.chats
            .iter()
            .any(|c| c.id == "19:meeting_elsewhere@thread.v2")
    );
}

#[test]
fn nine_to_fifteen_digits_only() {
    assert_eq!(
        meeting_id("123 456 789 012"),
        Some("123456789012".to_owned())
    );
    assert_eq!(meeting_id("123456789"), Some("123456789".to_owned()));
    assert_eq!(meeting_id("12345678"), None);
    assert!(meeting_id("123456789012345").is_some());
    assert_eq!(meeting_id("1234567890123456"), None);
    assert_eq!(meeting_id("12345678a"), None);
    assert_eq!(meeting_id(""), None);
    assert_eq!(meeting_id(UNKNOWN), None);
}

fn recording_app() -> (
    App,
    tempfile::TempDir,
    tokio::sync::mpsc::UnboundedReceiver<Command>,
) {
    let (mut app, dir) = demo();
    let (backend, commands) = crate::backend::Backend::recording();
    app.backend = backend;
    (app, dir, commands)
}

#[test]
fn id_needs_scope_then_sends_find() {
    let (mut app, _dir, mut commands) = recording_app();
    let ctx = egui::Context::default();
    app.demo = false;
    app.granted.clear();
    app.join_meeting("123 456 789".into(), &ctx);
    assert!(matches!(app.overlay, Some(Overlay::Consent(_))));
    assert!(commands.try_recv().is_err());

    app.overlay = None;
    app.granted = crate::auth::ONLINE_MEETINGS_SCOPE.to_owned();
    app.join_meeting("123 456 789".into(), &ctx);
    assert!(app.overlay.is_none());
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::FindMeetingById(id)) if id == "123456789"
    ));
    assert!(app.call.is_none());
}

#[test]
fn found_id_joins_like_link() {
    let (mut app, _dir) = demo();
    let ctx = egui::Context::default();
    app.on_meeting_by_id("123456789".into(), Some(UNKNOWN.into()), &ctx);
    let call = app.call.as_ref().expect("a call");
    assert_eq!(call.join_url(), Some(UNKNOWN));
}

#[test]
fn missing_id_toasts_paste_link() {
    let (mut app, _dir) = demo();
    let ctx = egui::Context::default();
    app.on_meeting_by_id("123456789".into(), None, &ctx);
    assert!(app.call.is_none());
    assert!(app.toasts.iter().any(|t| t.kind == ToastKind::Error
        && t.text == "No meeting found with that ID. Paste the meeting link instead."));
}

#[test]
fn calls_started_from_a_chat_follow_that_chat() {
    let (mut app, _dir) = demo();
    let ctx = egui::Context::default();
    let me = app.me().to_owned();
    let chat = app
        .chats
        .iter()
        .find(|c| c.kind == ChatKind::OneOnOne && c.peer(&me).is_some_and(|p| p.email.is_some()))
        .cloned()
        .unwrap();
    let email = chat.peer(&me).and_then(|p| p.email.clone()).unwrap();
    let name = chat.peer(&me).map(|p| p.display_name.clone()).unwrap();
    // From the Calls page: the one-on-one chat is found by address.
    app.section = Section::Calls;
    app.start_call(vec![email], true, &ctx);
    let call = app.call.as_ref().unwrap();
    assert_eq!(call.title(), name);
    assert_eq!(call.chat, Some(ConversationId::Chat(chat.id.clone())));
    assert_eq!(call.lookup, ChatLookup::NotApplicable);
}

#[test]
fn settings_reach_every_backend_and_profiles_stick_to_accounts() {
    let (mut app, _dir) = demo();
    app.id = Some("oid.tid".into());
    app.remember_browser_profile();
    let profile = app.browser_profile.clone();
    assert_eq!(app.settings.calls.profiles.get("oid.tid"), Some(&profile));
    assert!(app.browser_profile(0).is_none(), "off by default");
    let mut calls = app.settings.calls.clone();
    calls.profile_per_account = true;
    app.set_call_settings(calls);
    assert_eq!(app.browser_profile(0), Some(profile));
}

#[test]
fn links_open_in_the_call_browser_only_when_chosen() {
    let (mut app, _dir) = demo();
    let edge = Browser {
        kind: browser::BrowserKind::Edge,
        program: "/edge".into(),
    };
    let chrome = Browser {
        kind: browser::BrowserKind::Chrome,
        program: "/chrome".into(),
    };
    app.browsers = Some(vec![edge, chrome]);
    let url = "https://example.com/doc";
    assert!(
        app.link_launch(url).is_none(),
        "the default browser by default"
    );

    let mut calls = app.settings.calls.clone();
    calls.links_in_browser = true;
    calls.browser = Some(browser::BrowserKind::Chrome);
    app.set_call_settings(calls);
    let launch = app.link_launch(url).unwrap();
    assert_eq!(launch.program, std::path::PathBuf::from("/chrome"));
    assert_eq!(launch.args, [url]);
    // Links follow the call profile when each account has its own.
    let mut calls = app.settings.calls.clone();
    calls.profile_per_account = true;
    app.set_call_settings(calls);
    let profile = app.browser_profile.clone();
    assert_eq!(
        app.link_launch(url).unwrap().args,
        [format!("--profile-directory={profile}"), url.to_owned()]
    );
    // Links that are not web links never reach the browser.
    assert!(app.link_launch("file:///etc/passwd").is_none());
    // With no Edge or Chrome, the default browser opens them.
    app.browsers = Some(Vec::new());
    assert!(app.link_launch(url).is_none());
}

fn ada_chat(app: &App) -> ConversationId {
    let me = app.me().to_owned();
    let chat = app
        .chats
        .iter()
        .find(|c| c.kind == ChatKind::OneOnOne && c.peer(&me).is_some_and(|p| p.user_id == "u-ada"))
        .expect("the demo has a chat with Ada");
    ConversationId::Chat(chat.id.clone())
}

#[test]
fn a_ring_resolves_the_caller_from_their_object_id_and_raises_the_window() {
    let (mut app, _dir) = demo_app();
    app.wants_show = false;
    app.on_incoming_call(ring("call-1", "u-ada"));
    let shown = &app.rings[0];
    assert_eq!(shown.chat, Some(ada_chat(&app)));
    assert!(shown.email().is_some_and(|e| e.contains('@')));
    assert!(app.wants_show);
    // The same call again replaces the toast rather than stacking it.
    app.on_incoming_call(ring("call-1", "u-ada"));
    assert_eq!(app.rings.len(), 1);
}

#[test]
fn open_chat_from_a_ring_opens_the_callers_chat_and_closes_the_toast() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    app.section = Section::Calls;
    app.on_incoming_call(ring("call-1", "u-ada"));
    app.apply(Action::RingOpenChat("call-1".into()), &ctx);
    assert_eq!(app.open, Some(ada_chat(&app)));
    assert_eq!(app.section, Section::Chats);
    assert!(app.rings.is_empty());
}

#[test]
fn an_unknown_caller_cannot_open_a_chat_and_a_call_back_falls_back_to_teams() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    app.on_incoming_call(ring("call-2", "someone-new"));
    let shown = &app.rings[0];
    assert!(shown.member.is_none() && shown.chat.is_none());
    assert!(!shown.can_open_chat());
    app.apply(Action::RingPlaceCall("call-2".into()), &ctx);
    assert!(app.rings.is_empty());
    assert!(app.call.is_none());
    assert!(app.toasts.iter().any(|t| t.text.contains("opened Teams")));
}

#[test]
fn calling_back_a_group_ring_follows_the_callers_own_chat() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    let mut alert = ring("group-1", "u-ada");
    alert.group = true;
    alert.thread_id = Some(LAUNCH.into());
    app.on_incoming_call(alert);
    assert_eq!(app.rings[0].chat, Some(ConversationId::Chat(LAUNCH.into())));
    app.apply(Action::RingPlaceCall("group-1".into()), &ctx);
    let call = app.call.as_ref().expect("the call back starts");
    assert_eq!(call.chat, Some(ada_chat(&app)));
}

const FABRIKAM_MAE: &str = "19:fabrikam-mae@unq.gbl.spaces";

fn fabrikam_index(app: &App) -> usize {
    app.accounts
        .iter()
        .position(|a| a.id.as_deref() == Some(FABRIKAM))
        .expect("the demo has a Fabrikam account")
}

#[test]
fn a_ring_on_another_account_resolves_there_and_switches_to_it() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    let fabrikam = fabrikam_index(&app);
    assert_ne!(app.active, fabrikam);
    let mut alert = ring("f-call", "u-mae");
    alert.account = Some(FABRIKAM.into());
    app.on_incoming_call(alert);
    let shown = &app.rings[0];
    assert_eq!(shown.account.as_deref(), Some(FABRIKAM));
    assert_eq!(shown.account_name.as_deref(), Some("alex@fabrikam.com"));
    assert_eq!(shown.chat, Some(ConversationId::Chat(FABRIKAM_MAE.into())));
    assert_eq!(shown.email(), Some("mae@fabrikam.com"));
    let key = app.rings[0].id.clone();
    app.apply(Action::RingOpenChat(key), &ctx);
    assert_eq!(app.active, fabrikam);
    assert_eq!(app.open, Some(ConversationId::Chat(FABRIKAM_MAE.into())));
}

#[test]
fn a_ring_on_an_account_chaat_lacks_shows_unresolved() {
    let (mut app, _dir) = demo_app();
    let active = app.active;
    let mut alert = ring("x-call", "u-ada");
    alert.account = Some("other.tenant".into());
    alert.username = Some("someone@elsewhere.com".into());
    app.on_incoming_call(alert);
    let shown = &app.rings[0];
    assert_eq!(shown.account, None);
    assert_eq!(shown.account_name.as_deref(), Some("someone@elsewhere.com"));
    assert!(shown.member.is_none() && !shown.can_open_chat());
    let ctx = egui::Context::default();
    let key = app.rings[0].id.clone();
    app.apply(Action::RingPlaceCall(key), &ctx);
    assert_eq!(app.active, active, "no account to switch to");
}

#[test]
fn each_account_gets_its_own_sign_in_card() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    call_alerts_plugin(&mut app, "ca");
    let ask = |app: &App, account: &str, code: &str| {
        plugin_event(app, "ca", sign_in_event(Some(account), code))
    };
    app.handle(ask(&app, "u-me.demo", "ONE"), &ctx);
    app.handle(ask(&app, FABRIKAM, "TWO"), &ctx);
    app.handle(ask(&app, FABRIKAM, "THREE"), &ctx);
    assert_eq!(app.plugins["ca"].sign_in.len(), 2);
    let fabrikam = app.plugins["ca"]
        .sign_in
        .iter()
        .find(|s| s.account.as_deref() == Some(FABRIKAM))
        .unwrap();
    assert_eq!(fabrikam.user_code, "THREE");
    assert_eq!(fabrikam.username.as_deref(), Some("alex@fabrikam.com"));
    app.handle(
        plugin_event(&app, "ca", signed_in_event(Some(FABRIKAM))),
        &ctx,
    );
    assert_eq!(app.plugins["ca"].sign_in.len(), 1);
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
    assert_eq!(app.plugins["ca"].listening, vec![FABRIKAM.to_owned()]);
}

#[test]
fn one_call_ringing_two_accounts_shows_a_card_for_each() {
    let (mut app, _dir) = demo_app();
    for account in ["u-me.demo", FABRIKAM] {
        let mut alert = ring("same-call", "u-ada");
        alert.account = Some(account.into());
        app.on_incoming_call(alert);
    }
    assert_eq!(app.rings.len(), 2);
}

#[test]
fn a_ring_card_stays_as_long_as_the_plugin_says_the_call_rings() {
    let (mut app, _dir) = demo_app();
    let mut alert = ring("short", "u-ada");
    alert.ring_timeout = Some(std::time::Duration::from_secs(15));
    app.on_incoming_call(alert);
    app.on_incoming_call(ring("default", "u-ada"));
    assert_eq!(app.rings[0].shown_for, std::time::Duration::from_secs(15));
    assert_eq!(app.rings[1].shown_for, crate::calling::alerts::DEFAULT_RING);
}

#[test]
fn dismissing_a_ring_removes_only_that_one() {
    let (mut app, _dir) = demo_app();
    let ctx = egui::Context::default();
    app.on_incoming_call(ring("a", "u-ada"));
    app.on_incoming_call(ring("b", "u-grace"));
    app.apply(Action::DismissRing("a".into()), &ctx);
    assert_eq!(app.rings.len(), 1);
    assert_eq!(app.rings[0].id, "b");
}
