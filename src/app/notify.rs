//! App-side notification glue: which new messages become toasts, meeting
//! reminders, urgent repeats, clicks on toasts and "notify when available".
//!
//! Nothing here fetches from Graph. Everything works on data already in
//! memory, and nothing logs notification text, names or ids.

use std::collections::HashMap;
use std::time::Duration;

use super::{Account, App};
use crate::backend::AuthStatus;
use crate::model::{Action, Availability, ChatKind, ConversationId, Presence, Section};
use crate::notify::rules::{self, Environment, Incoming, Kind, NotifySettings};
use crate::notify::schedule::{self, PauseFor};
use crate::notify::{Button, Clicked, Notice, Target};

/// Meetings are announced this long before they start.
const REMINDER_LEAD: i64 = 5 * 60 * 1000;

/// One new message as the notification code sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeItem {
    pub message: String,
    pub author: Option<String>,
    pub text: String,
    pub mentions_me: bool,
    pub urgent: bool,
    pub created: i64,
}

/// A stable text key for a conversation (replace key, mute key).
fn conversation_key(conversation: &ConversationId) -> String {
    match conversation {
        ConversationId::Chat(id) => id.clone(),
        ConversationId::Channel { channel, .. } => channel.clone(),
    }
}

/// Whether the person is in a call, by presence activity.
fn presence_in_call(presence: Option<&Presence>) -> bool {
    crate::calling::session::in_call(presence.and_then(|p| p.activity.as_deref()), false)
}

impl App {
    /// The rules' view of the moment, for one conversation.
    fn notify_environment(&self, conversation: &ConversationId) -> Environment {
        Environment {
            enabled: self.settings.notifications,
            paused: schedule::is_paused(self.settings.notify.paused_until, crate::time::now()),
            looking: !self.hidden
                && self.window_focused
                && self.open.as_ref() == Some(conversation),
            dnd: self
                .my_presence
                .as_ref()
                .is_some_and(|p| p.availability == Availability::DoNotDisturb),
            in_call: presence_in_call(self.my_presence.as_ref()) || self.call.is_some(),
        }
    }

    fn notify_incoming(&self, conversation: &ConversationId, item: &NoticeItem) -> Incoming {
        let me = self.me();
        let (kind, participated) = match conversation {
            ConversationId::Chat(id) => {
                let kind = match self.chat(id).map(|c| c.kind) {
                    Some(ChatKind::Group) => Kind::Group,
                    Some(ChatKind::Meeting) => Kind::Meeting,
                    _ => Kind::Direct,
                };
                let spoke = self
                    .conversations
                    .get(conversation)
                    .is_some_and(|c| c.messages.iter().any(|m| m.is_from(me)));
                (kind, self.participated.contains(id) || spoke)
            }
            ConversationId::Channel { .. } => (Kind::Channel, false),
        };
        Incoming {
            kind,
            muted: self
                .settings
                .notify
                .muted_chats
                .contains(&conversation_key(conversation)),
            mentions_me: item.mentions_me,
            urgent: item.urgent,
            participated,
        }
    }

    /// The title of a conversation's notification: with several accounts,
    /// the account it is for follows.
    fn notify_title(&self, account: &Account, conversation: &ConversationId) -> (String, String) {
        let plain = account.title_of(conversation);
        let shown = if self.accounts.len() > 1 {
            format!("{plain} · {}", account.username())
        } else {
            plain.clone()
        };
        (plain, shown)
    }

    fn request_attention(&self) {
        if self.settings.notify.flash
            && !self.window_focused
            && let Some(ctx) = &self.notify_ctx
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
        }
    }

    /// Turns the new messages of one conversation into at most one toast.
    pub(super) fn notify_batch(&mut self, conversation: &ConversationId, items: Vec<NoticeItem>) {
        let account_id = self.id.clone().unwrap_or_default();
        let key = conversation_key(conversation);
        let newest_key = (account_id.clone(), key.clone());
        let shown_up_to = self.notified_newest.get(&newest_key).copied();
        let env = self.notify_environment(conversation);
        // Items already covered by a newer toast, or dropped by the rules,
        // are not marked seen: another path may carry better flags for them.
        let passing: Vec<NoticeItem> = items
            .into_iter()
            .filter(|item| shown_up_to.is_none_or(|shown| item.created > shown))
            .filter(|item| {
                let incoming = self.notify_incoming(conversation, item);
                rules::decide(&self.settings.notify, &incoming, &env)
            })
            .collect();
        let passing: Vec<NoticeItem> = passing
            .into_iter()
            .filter(|item| {
                self.seen
                    .first(&format!("{account_id}|{key}|{}", item.message))
            })
            .collect();
        let Some(newest) = passing.iter().max_by_key(|item| item.created) else {
            return;
        };
        self.notified_newest.insert(newest_key, newest.created);
        if self.settings.notify.urgent_repeat {
            let now = crate::time::now();
            for item in passing.iter().filter(|item| item.urgent) {
                self.urgent
                    .track(&account_id, conversation, &item.message, item.created, now);
            }
        }
        let account = &self.accounts[self.active];
        let (plain, title) = self.notify_title(account, conversation);
        let body = rules::body(
            self.settings.notify.preview,
            &plain,
            newest.author.as_deref(),
            &newest.text,
            passing.len() - 1,
        );
        self.desktop.show(Notice {
            title,
            body,
            target: Target::Conversation {
                account: self.id.clone(),
                conversation: conversation.clone(),
                message: Some(newest.message.clone()),
            },
            buttons: Vec::new(),
            replace: Some(key),
        });
        self.request_attention();
    }

    /// Meeting reminders from the cached calendars, and snoozed ones coming
    /// back at the meeting's start. No network involved.
    pub(super) fn remind(&mut self, ctx: &egui::Context) {
        if !self.settings.notifications {
            return;
        }
        let now = crate::time::now();
        if schedule::is_paused(self.settings.notify.paused_until, now) {
            return;
        }
        let mut next: Option<i64> = None;
        let mut shown = Vec::new();
        for account in &mut self.accounts {
            let mut due = Vec::new();
            for meeting in &account.meetings {
                if meeting.cancelled || meeting.all_day || account.reminded.contains(&meeting.id) {
                    continue;
                }
                let at = meeting.start - REMINDER_LEAD;
                if at <= now && meeting.start > now - 60_000 {
                    due.push(meeting.clone());
                } else if at > now {
                    next = Some(next.map_or(at, |n| n.min(at)));
                }
            }
            for meeting in due {
                account.reminded.insert(meeting.id.clone());
                shown.push((account.id.clone(), meeting));
            }
        }
        // Snoozed reminders come back once, at the meeting's start.
        let snoozed = std::mem::take(&mut self.snoozed);
        for (meeting, at) in snoozed {
            if at > now {
                next = Some(next.map_or(at, |n| n.min(at)));
                self.snoozed.push((meeting, at));
                continue;
            }
            let found = self.accounts.iter().find_map(|account| {
                account
                    .meetings
                    .iter()
                    .find(|m| m.id == meeting && !m.cancelled)
                    .map(|m| (account.id.clone(), m.clone()))
            });
            if let Some(found) = found {
                shown.push(found);
            }
        }
        for (account, meeting) in shown {
            let mut buttons = Vec::new();
            if meeting.join_url.is_some() {
                buttons.push(Button::Join);
            }
            if meeting.start > now {
                buttons.push(Button::Snooze);
            }
            self.desktop.show(Notice {
                title: meeting.subject.clone(),
                body: format!("Starts {}", crate::time::until_label(meeting.start, now)),
                target: Target::Meeting {
                    account,
                    meeting: meeting.id.clone(),
                },
                buttons,
                replace: Some(format!("meeting:{}", meeting.id)),
            });
        }
        if let Some(next) = next {
            ctx.request_repaint_after(Duration::from_millis(
                (next - now).clamp(1_000, 3_600_000) as u64
            ));
        }
    }

    /// Shows urgent messages again while their chat stays unread.
    pub(super) fn tick_urgent(&mut self, ctx: &egui::Context) {
        let now = crate::time::now();
        let notify = &self.settings.notify;
        let active = self.settings.notifications
            && notify.urgent_repeat
            && !schedule::is_paused(notify.paused_until, now);
        for repeat in self.urgent.due(now) {
            let Some(account) = self
                .accounts
                .iter()
                .find(|a| a.id.clone().unwrap_or_default() == repeat.account)
            else {
                continue;
            };
            let unread = match &repeat.conversation {
                ConversationId::Chat(id) => account
                    .chat(id)
                    .is_some_and(|chat| chat.is_unread(account.me())),
                ConversationId::Channel { .. } => false,
            };
            if !unread {
                self.urgent.stop(&repeat.account, &repeat.conversation, now);
                continue;
            }
            if !active {
                continue;
            }
            let (plain, title) = self.notify_title(account, &repeat.conversation);
            let message = account
                .conversations
                .get(&repeat.conversation)
                .and_then(|c| c.messages.iter().find(|m| m.id == repeat.message));
            let body = rules::body(
                self.settings.notify.preview,
                &plain,
                message.map(|m| m.author.as_str()),
                &message.map(|m| m.body.summary(140)).unwrap_or_default(),
                0,
            );
            self.desktop.show(Notice {
                title,
                body,
                target: Target::Conversation {
                    account: account.id.clone(),
                    conversation: repeat.conversation.clone(),
                    message: Some(repeat.message.clone()),
                },
                buttons: Vec::new(),
                replace: Some(conversation_key(&repeat.conversation)),
            });
            self.request_attention();
        }
        if let Some(next) = self.urgent.next_due() {
            ctx.request_repaint_after(Duration::from_millis(
                (next - now).clamp(1_000, 600_000) as u64
            ));
        }
    }

    /// The index of a signed-in account by its id.
    fn signed_in_account_index(&self, id: &str) -> Option<usize> {
        self.accounts.iter().position(|a| {
            a.id.as_deref() == Some(id) && matches!(a.auth, AuthStatus::SignedIn { .. })
        })
    }

    /// Acts on what the person did with toasts.
    pub(super) fn handle_clicks(&mut self) {
        for clicked in self.desktop.clicks() {
            let (Clicked::Open(target) | Clicked::Join(target) | Clicked::Snooze(target)) =
                &clicked;
            let account = match target {
                Target::Conversation { account, .. }
                | Target::Meeting { account, .. }
                | Target::Person { account, .. } => account.clone(),
                Target::Test => None,
            };
            let index = match account {
                Some(id) => match self.signed_in_account_index(&id) {
                    Some(index) => index,
                    None => continue,
                },
                None => self.active,
            };
            if index != self.active && !matches!(clicked, Clicked::Snooze(_)) {
                self.actions.push(Action::SwitchAccount(index));
            }
            match clicked {
                Clicked::Open(target) => {
                    match target {
                        Target::Conversation {
                            conversation,
                            message: Some(message),
                            ..
                        } => self.actions.push(Action::OpenMessage {
                            conversation,
                            message,
                        }),
                        Target::Conversation { conversation, .. } => {
                            self.actions.push(Action::OpenConversation(conversation));
                        }
                        Target::Meeting { .. } => {
                            self.actions.push(Action::Navigate(Section::Calendar));
                        }
                        Target::Person { .. } | Target::Test => {}
                    }
                    self.wants_show = true;
                }
                Clicked::Join(Target::Meeting { meeting, .. }) => {
                    let url = self.accounts[index]
                        .meetings
                        .iter()
                        .find(|m| m.id == meeting)
                        .and_then(|m| m.join_url.clone());
                    if let Some(url) = url {
                        let chat = self.accounts[index]
                            .chats
                            .iter()
                            .find(|c| {
                                c.join_url
                                    .as_deref()
                                    .is_some_and(|j| crate::calling::same_meeting(j, &url))
                            })
                            .map(|c| c.id.clone());
                        if let Some(chat) = chat {
                            self.participated.insert(chat);
                        }
                        self.actions.push(Action::JoinMeeting(url));
                    }
                }
                Clicked::Snooze(Target::Meeting { meeting, .. }) => {
                    let start = self.accounts[index]
                        .meetings
                        .iter()
                        .find(|m| m.id == meeting)
                        .map(|m| m.start);
                    if let Some(start) = start {
                        self.snoozed.retain(|(id, _)| *id != meeting);
                        self.snoozed.push((meeting, start));
                    }
                }
                Clicked::Join(_) | Clicked::Snooze(_) => {}
            }
        }
    }

    /// "Notify when available": a toast when a followed person's presence
    /// moves to Available from something else.
    pub(super) fn observe_presence_for_follow(&mut self, before: &HashMap<String, Presence>) {
        let now = crate::time::now();
        let notify = &self.settings.notify;
        if notify.follow_available.is_empty()
            || !self.settings.notifications
            || schedule::is_paused(notify.paused_until, now)
        {
            return;
        }
        let dnd = notify.quiet_when_dnd
            && self
                .my_presence
                .as_ref()
                .is_some_and(|p| p.availability == Availability::DoNotDisturb);
        if dnd {
            return;
        }
        let mut shown = false;
        for (user, name) in &notify.follow_available {
            let became = self
                .presence
                .get(user)
                .is_some_and(|p| p.availability == Availability::Available)
                && before
                    .get(user)
                    .is_some_and(|p| p.availability != Availability::Available);
            if became {
                self.desktop.show(Notice {
                    title: format!("{name} is available"),
                    body: "Now available in Teams".into(),
                    target: Target::Person {
                        account: self.id.clone(),
                        user: user.clone(),
                    },
                    buttons: Vec::new(),
                    replace: Some(format!("available:{user}")),
                });
                shown = true;
            }
        }
        if shown {
            self.request_attention();
        }
    }

    pub(super) fn set_notify(&mut self, settings: NotifySettings) {
        self.settings.notify = settings;
        self.mark_settings_dirty();
    }

    pub(super) fn mute_chat(&mut self, chat: String, muted: bool) {
        let list = &mut self.settings.notify.muted_chats;
        let changed = if muted {
            list.insert(chat)
        } else {
            list.remove(&chat)
        };
        if changed {
            self.mark_settings_dirty();
        }
    }

    pub(super) fn follow_available(&mut self, user: String, name: String, on: bool) {
        let list = &mut self.settings.notify.follow_available;
        if on {
            list.insert(user, name);
        } else {
            list.remove(&user);
        }
        self.mark_settings_dirty();
    }

    pub(super) fn pause_notifications(&mut self, choice: Option<PauseFor>) {
        self.settings.notify.paused_until =
            choice.map(|choice| schedule::pause_until(choice, crate::time::now()));
        self.mark_settings_dirty();
    }

    /// A fixed test toast that bypasses the rules.
    pub(super) fn send_test_notification(&mut self) {
        self.desktop.show(Notice {
            title: "Chaat".into(),
            body: "Notifications are working.".into(),
            target: Target::Test,
            buttons: Vec::new(),
            replace: Some("test".into()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Event, Merge, Waker};
    use crate::model::{Chat, Meeting, Message};
    use crate::notify::Clicked;
    use crate::paths::AppDirs;
    use crate::settings::Settings;

    const LAUNCH: &str = "19:demo-launch@thread.v2";
    const ADA: &str = "19:demo-ada@unq.gbl.spaces";
    const SECOND_CHAT: &str = "19:fabrikam-mae@unq.gbl.spaces";

    fn demo() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(
            AppDirs::under(dir.path()),
            Settings::default(),
            Waker::default(),
            true,
        );
        app.settings.plugins.clear();
        app.plugins.clear();
        app.settings.notifications = true;
        app.open = None;
        app.window_focused = true;
        (app, dir)
    }

    fn chat(id: &str) -> ConversationId {
        ConversationId::Chat(id.into())
    }

    fn item(id: &str, created: i64) -> NoticeItem {
        NoticeItem {
            message: id.into(),
            author: Some("Ada".into()),
            text: format!("text {id}"),
            mentions_me: false,
            urgent: false,
            created,
        }
    }

    #[test]
    fn batch_is_one_toast_with_more() {
        let (mut app, _dir) = demo();
        app.notify_batch(
            &chat(LAUNCH),
            vec![item("a", 1), item("c", 3), item("b", 2)],
        );
        let shown = app.desktop.recorded();
        assert_eq!(shown.len(), 1);
        assert!(shown[0].body.contains("text c"), "{}", shown[0].body);
        assert!(shown[0].body.contains("+2 more"), "{}", shown[0].body);
        assert_eq!(shown[0].replace.as_deref(), Some(LAUNCH));
        assert!(matches!(
            &shown[0].target,
            Target::Conversation { message: Some(m), .. } if m == "c"
        ));
    }

    #[test]
    fn same_message_twice_is_one_toast() {
        let (mut app, _dir) = demo();
        app.notify_batch(&chat(LAUNCH), vec![item("a", 1)]);
        app.notify_batch(&chat(LAUNCH), vec![item("a", 1)]);
        assert_eq!(app.desktop.recorded().len(), 1);
    }

    #[test]
    fn urgent_dropped_by_chat_list_still_toasts_from_messages() {
        let (mut app, _dir) = demo();
        app.my_presence = Some(Presence {
            availability: Availability::DoNotDisturb,
            ..Default::default()
        });
        app.notify_batch(&chat(LAUNCH), vec![item("u", 5)]);
        assert!(app.desktop.recorded().is_empty());
        let mut urgent = item("u", 5);
        urgent.urgent = true;
        app.notify_batch(&chat(LAUNCH), vec![urgent]);
        assert_eq!(app.desktop.recorded().len(), 1);
    }

    #[test]
    fn mention_dropped_by_chat_list_still_toasts_from_messages() {
        let (mut app, _dir) = demo();
        app.settings.notify.group_chats = rules::GroupAlerts::Mentions;
        app.notify_batch(&chat(LAUNCH), vec![item("m", 5)]);
        assert!(app.desktop.recorded().is_empty());
        let mut mention = item("m", 5);
        mention.mentions_me = true;
        app.notify_batch(&chat(LAUNCH), vec![mention]);
        assert_eq!(app.desktop.recorded().len(), 1);
    }

    #[test]
    fn older_messages_after_newer_toast_stay_quiet() {
        let (mut app, _dir) = demo();
        app.notify_batch(&chat(LAUNCH), vec![item("c", 3)]);
        app.notify_batch(
            &chat(LAUNCH),
            vec![item("a", 1), item("b", 2), item("c", 3)],
        );
        let shown = app.desktop.recorded();
        assert_eq!(shown.len(), 1);
        assert!(shown[0].body.contains("text c"), "{}", shown[0].body);
        assert!(!shown[0].body.contains("more"), "{}", shown[0].body);
    }

    #[test]
    fn snooze_click_for_background_account_keeps_active() {
        let (mut app, _dir) = demo();
        let second = app.accounts[1].id.clone().unwrap();
        app.desktop.click(Clicked::Snooze(Target::Meeting {
            account: Some(second),
            meeting: "mtg".into(),
        }));
        app.handle_clicks();
        assert!(
            !app.actions
                .iter()
                .any(|a| matches!(a, Action::SwitchAccount(_)))
        );
        assert_eq!(app.active, 0);
    }

    #[test]
    fn dnd_silences_but_urgent_passes() {
        let (mut app, _dir) = demo();
        app.my_presence = Some(Presence {
            availability: Availability::DoNotDisturb,
            ..Default::default()
        });
        app.notify_batch(&chat(LAUNCH), vec![item("a", 1)]);
        assert!(app.desktop.recorded().is_empty());
        let mut urgent = item("b", 2);
        urgent.urgent = true;
        app.notify_batch(&chat(LAUNCH), vec![urgent]);
        assert_eq!(app.desktop.recorded().len(), 1);
    }

    #[test]
    fn open_focused_chat_stays_quiet() {
        let (mut app, _dir) = demo();
        app.open = Some(chat(LAUNCH));
        app.notify_batch(&chat(LAUNCH), vec![item("a", 1)]);
        assert!(app.desktop.recorded().is_empty());
        app.window_focused = false;
        app.notify_batch(&chat(LAUNCH), vec![item("b", 2)]);
        assert_eq!(app.desktop.recorded().len(), 1);
    }

    #[test]
    fn click_switches_account_and_opens() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        assert!(app.accounts.len() > 1);
        let second = app.accounts[1].id.clone().unwrap();
        // A click for an unknown account does nothing.
        app.desktop.click(Clicked::Open(Target::Conversation {
            account: Some("u-nobody.none".into()),
            conversation: chat(SECOND_CHAT),
            message: None,
        }));
        app.handle_clicks();
        app.apply_actions(&ctx);
        assert_eq!(app.active, 0);
        assert!(!app.wants_show);

        app.desktop.click(Clicked::Open(Target::Conversation {
            account: Some(second),
            conversation: chat(SECOND_CHAT),
            message: Some("m1".into()),
        }));
        app.handle_clicks();
        app.apply_actions(&ctx);
        assert_eq!(app.active, 1);
        assert_eq!(app.open, Some(chat(SECOND_CHAT)));
        assert_eq!(app.jump_to.as_deref(), Some("m1"));
        assert!(app.wants_show);
    }

    fn meeting(start: i64) -> Meeting {
        Meeting {
            id: "mtg".into(),
            subject: "Standup".into(),
            start,
            end: start + 1_800_000,
            join_url: Some("https://teams.microsoft.com/l/meetup-join/x".into()),
            ..Default::default()
        }
    }

    #[test]
    fn snooze_reshows_at_start() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        let now = crate::time::now();
        app.meetings = vec![meeting(now + 240_000)];
        app.remind(&ctx);
        let shown = app.desktop.recorded();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].buttons, vec![Button::Join, Button::Snooze]);

        app.desktop.click(Clicked::Snooze(shown[0].target.clone()));
        app.handle_clicks();
        assert_eq!(app.snoozed, vec![("mtg".to_owned(), now + 240_000)]);
        app.remind(&ctx);
        assert_eq!(app.desktop.recorded().len(), 1, "not before the start");

        app.snoozed[0].1 = now - 1;
        app.meetings[0].start = now - 1;
        app.remind(&ctx);
        let shown = app.desktop.recorded();
        assert_eq!(shown.len(), 2);
        assert_eq!(
            shown[1].buttons,
            vec![Button::Join],
            "no Snooze after start"
        );
        assert!(app.snoozed.is_empty());
    }

    #[test]
    fn join_click_joins_and_marks_participation() {
        let (mut app, _dir) = demo();
        let now = crate::time::now();
        let m = meeting(now + 60_000);
        let url = m.join_url.clone().unwrap();
        app.meetings = vec![m];
        app.chats.push(Chat {
            id: "19:meeting_x@thread.v2".into(),
            kind: ChatKind::Meeting,
            join_url: Some(url.clone()),
            ..Default::default()
        });
        app.desktop.click(Clicked::Join(Target::Meeting {
            account: app.id.clone(),
            meeting: "mtg".into(),
        }));
        app.handle_clicks();
        assert!(app.participated.contains("19:meeting_x@thread.v2"));
        assert!(
            app.actions
                .iter()
                .any(|a| matches!(a, Action::JoinMeeting(u) if *u == url))
        );
    }

    fn presence(availability: Availability) -> Presence {
        Presence {
            availability,
            ..Default::default()
        }
    }

    #[test]
    fn follow_fires_on_transition_to_available_only() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        app.follow_available("u-ada".into(), "Ada".into(), true);
        let set = |app: &mut App, a| {
            app.handle(Event::Presence(vec![("u-ada".into(), presence(a))]), &ctx)
        };
        set(&mut app, Availability::Busy);
        set(&mut app, Availability::Away);
        assert!(app.desktop.recorded().is_empty());
        set(&mut app, Availability::Available);
        let shown = app.desktop.recorded();
        assert_eq!(shown.len(), 1);
        assert!(shown[0].title.contains("Ada"));
        assert!(matches!(shown[0].target, Target::Person { .. }));
        set(&mut app, Availability::Available);
        assert_eq!(app.desktop.recorded().len(), 1);
    }

    #[test]
    fn flash_requests_attention_when_unfocused() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        app.notify_ctx = Some(ctx.clone());
        let attention = |ctx: &egui::Context| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
            out.textures_delta.clear();
            out.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::RequestUserAttention(_)))
        };
        app.window_focused = true;
        app.notify_batch(&chat(LAUNCH), vec![item("a", 1)]);
        assert!(!attention(&ctx), "focused window needs no flash");
        app.window_focused = false;
        app.notify_batch(&chat(LAUNCH), vec![item("b", 2)]);
        assert!(attention(&ctx));
        app.settings.notify.flash = false;
        app.notify_batch(&chat(LAUNCH), vec![item("c", 3)]);
        assert!(!attention(&ctx));
    }

    #[test]
    fn chat_list_and_messages_paths_feed_notify_batch() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        app.window_focused = false;
        // The chat list path.
        let mut chats = app.chats.clone();
        let ada = chats.iter_mut().find(|c| c.id == ADA).unwrap();
        ada.preview = Some(crate::model::Preview {
            message_id: "list-1".into(),
            author_id: Some("u-ada".into()),
            author: Some("Ada".into()),
            text: "from the list".into(),
            at: i64::MAX / 2,
            ..Default::default()
        });
        app.handle(
            Event::Chats {
                chats,
                cached: false,
            },
            &ctx,
        );
        let shown = app.desktop.recorded();
        assert_eq!(shown.len(), 1);
        assert!(shown[0].body.contains("from the list"));

        // The messages path: one toast for the whole batch.
        let base = crate::time::now() + 10_000;
        let incoming = |id: &str, at: i64| Message {
            id: id.into(),
            created: at,
            author: "Grace".into(),
            author_id: Some("u-grace".into()),
            body: crate::html::RichText::plain(id),
            ..Default::default()
        };
        let batch = |app: &mut App, messages| {
            app.handle(
                Event::Messages {
                    conversation: chat(LAUNCH),
                    messages,
                    merge: Merge::Latest,
                    more: None,
                    cached: false,
                },
                &ctx,
            )
        };
        batch(&mut app, vec![incoming("m0", base)]);
        let before = app.desktop.recorded().len();
        let mut mention = incoming("m1", base + 1);
        mention.mentions = vec![app.me().to_owned()];
        batch(&mut app, vec![incoming("m2", base + 2), mention]);
        assert_eq!(app.desktop.recorded().len(), before + 1, "one per batch");
        let last = app.desktop.recorded().pop().unwrap();
        assert_eq!(last.replace.as_deref(), Some(LAUNCH));
        assert!(last.body.contains("+1 more"), "{}", last.body);
    }

    #[test]
    fn mute_action_saves_settings() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        app.settings_dirty = false;
        app.apply(Action::MuteChat(LAUNCH.into(), true), &ctx);
        assert!(app.settings.notify.muted_chats.contains(LAUNCH));
        assert!(app.settings_dirty);
        app.notify_batch(&chat(LAUNCH), vec![item("a", 1)]);
        assert!(app.desktop.recorded().is_empty());
        app.apply(Action::MuteChat(LAUNCH.into(), false), &ctx);
        assert!(app.settings.notify.muted_chats.is_empty());
    }

    #[test]
    fn pause_action_sets_until() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        let before = crate::time::now();
        app.apply(Action::PauseNotifications(Some(PauseFor::OneHour)), &ctx);
        let until = app.settings.notify.paused_until.unwrap();
        assert!(until >= before);
        app.apply(Action::PauseNotifications(None), &ctx);
        assert_eq!(app.settings.notify.paused_until, None);
    }

    #[test]
    fn test_notification_bypasses_rules() {
        let (mut app, _dir) = demo();
        app.settings.notifications = false;
        app.send_test_notification();
        let shown = app.desktop.recorded();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].target, Target::Test);
    }

    #[test]
    fn urgent_repeat_stops_when_chat_is_opened() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        let now = crate::time::now();
        let mut urgent = item("u1", now);
        urgent.urgent = true;
        app.notify_batch(&chat(LAUNCH), vec![urgent]);
        app.apply(Action::OpenConversation(chat(LAUNCH)), &ctx);
        assert!(app.urgent.due(now + 130_000).is_empty());
    }

    #[test]
    fn urgent_repeat_fires_after_two_minutes_while_unread() {
        let (mut app, _dir) = demo();
        let ctx = egui::Context::default();
        let oncall = chat("19:demo-oncall@thread.v2");
        let id = app.id.clone().unwrap_or_default();
        // No clock seam: track as if the message arrived 130 s ago, so its
        // first repeat slot (created + 2 min) has passed.
        let now = crate::time::now();
        let created = now - 130_000;
        app.urgent.track(&id, &oncall, "u-hedy", created, created);
        app.tick_urgent(&ctx);
        let shown = app.desktop.recorded();
        assert_eq!(shown.len(), 1, "one repeat toast");
        assert_eq!(
            shown[0].replace.as_deref(),
            Some("19:demo-oncall@thread.v2")
        );
    }
}
