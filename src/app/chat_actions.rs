//! Hiding chats, marking them unread, and the Undo on the toast.

use super::App;
use crate::backend::AccountCommand;
use crate::model::{Chat, Toast, ToastAction, ToastKind};

impl App {
    pub(super) fn hide_chat(&mut self, chat: String) {
        let Some(entry) = self.chats.iter_mut().find(|c| c.id == chat) else {
            return;
        };
        entry.hidden = true;
        self.pending_hide.insert(chat.clone(), true);
        self.undo_toast(chat.clone());
        self.send_account(AccountCommand::HideChat { chat, hide: true });
    }

    /// One toast per hidden chat, never merged with an earlier one, so each
    /// Undo keeps its own chat and account.
    fn undo_toast(&mut self, chat: String) {
        self.next_toast += 1;
        let id = self.next_toast;
        self.toasts.push(Toast {
            id,
            kind: ToastKind::Info,
            text: "Chat hidden".into(),
            action: Some(ToastAction::UndoHide(chat)),
            account: None,
        });
        self.toast_accounts.insert(id, self.id.clone());
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        let live: Vec<u64> = self.toasts.iter().map(|t| t.id).collect();
        self.toast_accounts.retain(|id, _| live.contains(id));
    }

    pub(super) fn unhide_chat(&mut self, chat: String) {
        if let Some(entry) = self.chats.iter_mut().find(|c| c.id == chat) {
            entry.hidden = false;
        }
        self.pending_hide.insert(chat.clone(), false);
        self.send_account(AccountCommand::HideChat { chat, hide: false });
    }

    pub(super) fn mark_chat_unread(&mut self, chat: String) {
        let Some(entry) = self.chats.iter_mut().find(|c| c.id == chat) else {
            return;
        };
        entry.marked_unread = true;
        self.send_account(AccountCommand::MarkChatUnread(chat));
    }

    pub(super) fn mark_chat_read(&mut self, chat: String) {
        let was_marked = self
            .chats
            .iter_mut()
            .find(|c| c.id == chat)
            .is_some_and(|entry| std::mem::take(&mut entry.marked_unread));
        if was_marked && !self.hidden {
            // The preview may already count as read, but Graph was told the
            // chat is unread; say it is read again.
            if let Some(entry) = self.chats.iter_mut().find(|c| c.id == chat) {
                entry.last_read = Some(crate::time::now());
            }
            self.send_account(AccountCommand::MarkRead(chat));
            return;
        }
        self.mark_read(&chat);
    }

    pub(super) fn run_toast_action(&mut self, toast: u64) {
        let Some(found) = self.toasts.iter().find(|t| t.id == toast) else {
            return;
        };
        let action = found.action.clone();
        self.toasts.retain(|t| t.id != toast);
        let account = self.toast_accounts.remove(&toast);
        match action {
            Some(ToastAction::UndoHide(chat)) => {
                // The person may have switched accounts since the hide.
                let target = account.and_then(|id| self.accounts.iter().position(|a| a.id == id));
                if let Some(index) = target {
                    self.in_account(index, |app| app.unhide_chat(chat));
                }
            }
            None => {}
        }
    }

    pub(super) fn on_chat_hidden(&mut self, chat: String, hidden: bool, ok: bool) {
        // Only the latest request settles the pending state: an older reply
        // (hide's, after Undo) must not drop the unhide in flight.
        let latest = self.pending_hide.get(&chat).is_none_or(|&h| h == hidden);
        if latest {
            self.pending_hide.remove(&chat);
        }
        if ok || !latest {
            return;
        }
        if let Some(entry) = self.chats.iter_mut().find(|c| c.id == chat) {
            entry.hidden = !hidden;
        }
        let text = if hidden {
            "Couldn't hide the chat."
        } else {
            "Couldn't unhide the chat."
        };
        self.toast(ToastKind::Error, text);
    }

    pub(super) fn on_mark_unread_failed(&mut self, chat: String) {
        if let Some(entry) = self.chats.iter_mut().find(|c| c.id == chat) {
            entry.marked_unread = false;
        }
        self.toast(ToastKind::Error, "Couldn't mark the chat as unread.");
    }

    /// Keeps what the person set locally (a pending hide, marked unread)
    /// over a chat list that may be older than the change.
    pub(super) fn keep_local_chat_flags(&self, chats: &mut [Chat]) {
        for incoming in chats.iter_mut() {
            if let Some(existing) = self.chats.iter().find(|c| c.id == incoming.id) {
                let newer = match (&incoming.preview, &existing.preview) {
                    (Some(new), Some(old)) => new.at > old.at,
                    (Some(_), None) => true,
                    _ => false,
                };
                if existing.marked_unread && !newer {
                    incoming.marked_unread = true;
                }
            }
            if let Some(&hidden) = self.pending_hide.get(&incoming.id) {
                incoming.hidden = hidden;
            }
        }
    }

    /// Whether the chat stays unread (marked unread while open), so
    /// opening it does not mark it read.
    pub(super) fn keeps_unread(&self, chat: &str) -> bool {
        self.chat(chat).is_some_and(|c| c.marked_unread)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppDirs, Waker};
    use crate::backend::{AccountCommand, AccountEvent, AuthStatus, Backend, Command};
    use crate::model::{ChatKind, ConversationId, Preview};
    use crate::settings::Settings;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn group_app() -> (App, UnboundedReceiver<Command>) {
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
        app.chats = vec![chat("19:a", 10), chat("19:b", 20)];
        (app, commands)
    }

    fn chat(id: &str, at: i64) -> Chat {
        Chat {
            id: id.into(),
            kind: ChatKind::Group,
            preview: Some(Preview {
                message_id: format!("m{at}"),
                author_id: Some("u-ada".into()),
                author: Some("Ada".into()),
                text: "hi".into(),
                at,
                deleted: false,
                system: false,
                membership: None,
            }),
            last_read: Some(at),
            ..Default::default()
        }
    }

    fn drain(commands: &mut UnboundedReceiver<Command>) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(c) = commands.try_recv() {
            out.push(c);
        }
        out
    }

    fn get<'a>(app: &'a App, id: &str) -> &'a Chat {
        app.chat(id).unwrap()
    }

    #[test]
    fn hide_is_optimistic_with_undo_toast() {
        let (mut app, mut commands) = group_app();
        app.hide_chat("19:a".into());
        assert!(get(&app, "19:a").hidden);
        let toast = app.toasts.last().unwrap();
        assert_eq!(toast.text, "Chat hidden");
        assert_eq!(toast.action, Some(ToastAction::UndoHide("19:a".into())));
        assert!(
            drain(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::Account(AccountCommand::HideChat { chat, hide: true }) if chat == "19:a"))
        );
    }

    #[test]
    fn undo_unhides() {
        let (mut app, mut commands) = group_app();
        app.hide_chat("19:a".into());
        drain(&mut commands);
        let id = app.toasts.last().unwrap().id;
        app.run_toast_action(id);
        assert!(!get(&app, "19:a").hidden);
        assert!(app.toasts.is_empty());
        assert_eq!(app.pending_hide.get("19:a"), Some(&false));
        assert!(
            drain(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::Account(AccountCommand::HideChat { chat, hide: false }) if chat == "19:a"))
        );
    }

    #[test]
    fn hide_failure_reverts_and_errors() {
        let (mut app, _commands) = group_app();
        app.hide_chat("19:a".into());
        app.on_chat_hidden("19:a".into(), true, false);
        assert!(!get(&app, "19:a").hidden);
        assert!(app.pending_hide.is_empty());
        let toast = app.toasts.last().unwrap();
        assert_eq!(toast.kind, ToastKind::Error);
        assert_eq!(toast.text, "Couldn't hide the chat.");

        app.unhide_chat("19:a".into());
        app.on_chat_hidden("19:a".into(), false, false);
        assert!(get(&app, "19:a").hidden);
        assert_eq!(app.toasts.last().unwrap().text, "Couldn't unhide the chat.");
    }

    #[test]
    fn hide_success_keeps_state_and_clears_pending() {
        let (mut app, _commands) = group_app();
        app.hide_chat("19:a".into());
        app.on_chat_hidden("19:a".into(), true, true);
        assert!(get(&app, "19:a").hidden);
        assert!(app.pending_hide.is_empty());
    }

    #[test]
    fn refresh_keeps_pending_hide() {
        let (mut app, _commands) = group_app();
        app.hide_chat("19:a".into());
        app.on_chats(vec![chat("19:a", 10), chat("19:b", 20)], false);
        assert!(get(&app, "19:a").hidden);
        assert!(!get(&app, "19:b").hidden);
    }

    #[test]
    fn mark_unread_survives_refresh() {
        let (mut app, mut commands) = group_app();
        app.mark_chat_unread("19:a".into());
        assert!(drain(&mut commands).iter().any(
            |c| matches!(c, Command::Account(AccountCommand::MarkChatUnread(id)) if id == "19:a")
        ));
        // A stale list: same preview, higher last_read.
        let mut stale = chat("19:a", 10);
        stale.last_read = Some(99);
        app.on_chats(vec![stale, chat("19:b", 20)], false);
        assert!(get(&app, "19:a").marked_unread);
        assert!(get(&app, "19:a").is_unread("u-me"));
    }

    #[test]
    fn new_message_clears_marked_unread() {
        let (mut app, _commands) = group_app();
        app.mark_chat_unread("19:a".into());
        app.on_chats(vec![chat("19:a", 50), chat("19:b", 20)], false);
        assert!(!get(&app, "19:a").marked_unread);
    }

    #[test]
    fn opening_clears_marked_unread() {
        let (mut app, mut commands) = group_app();
        app.mark_chat_unread("19:a".into());
        drain(&mut commands);
        app.open_conversation(ConversationId::Chat("19:a".into()));
        assert!(!get(&app, "19:a").marked_unread);
    }

    #[test]
    fn open_chat_marked_unread_stays_until_switch() {
        let (mut app, mut commands) = group_app();
        app.open_conversation(ConversationId::Chat("19:a".into()));
        drain(&mut commands);
        app.mark_chat_unread("19:a".into());
        drain(&mut commands);
        assert!(app.keeps_unread("19:a"));
        app.mark_read("19:a");
        assert!(
            !drain(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::Account(AccountCommand::MarkRead(_))))
        );
        assert!(get(&app, "19:a").marked_unread);
        // Another chat, then back: opening clears it.
        app.open_conversation(ConversationId::Chat("19:b".into()));
        assert!(get(&app, "19:a").marked_unread);
        app.open_conversation(ConversationId::Chat("19:a".into()));
        assert!(!get(&app, "19:a").marked_unread);
    }

    #[test]
    fn mark_read_clears_flag_and_tells_graph() {
        let (mut app, mut commands) = group_app();
        app.mark_chat_unread("19:a".into());
        drain(&mut commands);
        app.mark_chat_read("19:a".into());
        assert!(!get(&app, "19:a").marked_unread);
        assert!(
            drain(&mut commands).iter().any(
                |c| matches!(c, Command::Account(AccountCommand::MarkRead(id)) if id == "19:a")
            )
        );
    }

    #[test]
    fn mark_unread_failure_clears_flag_and_errors() {
        let (mut app, _commands) = group_app();
        app.mark_chat_unread("19:a".into());
        app.on_mark_unread_failed("19:a".into());
        assert!(!get(&app, "19:a").marked_unread);
        assert_eq!(
            app.toasts.last().unwrap().text,
            "Couldn't mark the chat as unread."
        );
    }

    #[test]
    fn two_hides_keep_two_undos() {
        let (mut app, mut commands) = group_app();
        app.hide_chat("19:a".into());
        app.hide_chat("19:b".into());
        drain(&mut commands);
        assert_eq!(app.toasts.len(), 2);
        let ids: Vec<u64> = app.toasts.iter().map(|t| t.id).collect();
        assert_ne!(ids[0], ids[1]);
        for id in ids {
            app.run_toast_action(id);
        }
        assert!(!get(&app, "19:a").hidden);
        assert!(!get(&app, "19:b").hidden);
        let sent = drain(&mut commands);
        for chat in ["19:a", "19:b"] {
            assert!(
                sent.iter().any(
                    |c| matches!(c, Command::Account(AccountCommand::HideChat { chat: id, hide: false }) if id == chat)
                ),
                "no unhide for {chat}"
            );
        }
    }

    #[test]
    fn opening_a_marked_unread_chat_tells_graph() {
        let (mut app, mut commands) = group_app();
        app.mark_chat_unread("19:a".into());
        drain(&mut commands);
        app.open_conversation(ConversationId::Chat("19:a".into()));
        assert!(!get(&app, "19:a").marked_unread);
        assert!(
            drain(&mut commands).iter().any(
                |c| matches!(c, Command::Account(AccountCommand::MarkRead(id)) if id == "19:a")
            )
        );
    }

    #[test]
    fn late_hide_reply_keeps_the_unhide_pending() {
        let (mut app, _commands) = group_app();
        app.hide_chat("19:a".into());
        let id = app.toasts.last().unwrap().id;
        app.run_toast_action(id);
        app.on_chat_hidden("19:a".into(), true, true);
        assert_eq!(app.pending_hide.get("19:a"), Some(&false));
        let mut stale = chat("19:a", 10);
        stale.hidden = true;
        app.on_chats(vec![stale, chat("19:b", 20)], false);
        assert!(!get(&app, "19:a").hidden);
        // The unhide's own reply settles it.
        app.on_chat_hidden("19:a".into(), false, true);
        assert!(app.pending_hide.is_empty());
    }

    #[test]
    fn late_hide_failure_does_not_undo_the_unhide() {
        let (mut app, _commands) = group_app();
        app.hide_chat("19:a".into());
        let id = app.toasts.last().unwrap().id;
        app.run_toast_action(id);
        app.on_chat_hidden("19:a".into(), true, false);
        assert!(!get(&app, "19:a").hidden);
        assert_eq!(app.pending_hide.get("19:a"), Some(&false));
        assert!(app.toasts.iter().all(|t| t.kind != ToastKind::Error));
    }

    #[test]
    fn undo_after_switching_accounts_unhides_on_the_hiding_account() {
        let (mut app, mut commands) = group_app();
        app.accounts[0].id = Some("u-me.t1".into());
        let (backend, mut second_commands) = Backend::recording();
        app.accounts.push(crate::app::Account::new(backend));
        app.accounts[1].id = Some("u-me2.t2".into());
        app.accounts[1].chats = vec![chat("19:c", 5)];
        app.hide_chat("19:a".into());
        drain(&mut commands);
        let id = app.toasts.last().unwrap().id;
        app.active = 1;
        app.run_toast_action(id);
        assert_eq!(app.active, 1);
        assert!(!app.accounts[0].chats[0].hidden);
        assert_eq!(app.accounts[0].pending_hide.get("19:a"), Some(&false));
        assert!(app.accounts[1].pending_hide.is_empty());
        assert!(
            drain(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::Account(AccountCommand::HideChat { chat, hide: false }) if chat == "19:a"))
        );
        assert!(drain(&mut second_commands).is_empty());
    }

    #[test]
    fn background_account_events_land_on_their_account() {
        let (mut app, _commands) = group_app();
        let (backend, _second_commands) = Backend::recording();
        app.accounts.push(crate::app::Account::new(backend));
        app.accounts[1].chats = vec![chat("19:c", 5)];
        let ctx = egui::Context::default();
        app.active = 1;
        app.hidden = true;
        app.handle(
            AccountEvent::Pins {
                chat: "19:c".into(),
                pins: Vec::new(),
            },
            &ctx,
        );
        app.handle(AccountEvent::MarkUnreadFailed("19:c".into()), &ctx);
        app.handle(
            AccountEvent::ChatHidden {
                chat: "19:c".into(),
                hidden: true,
                ok: false,
            },
            &ctx,
        );
        app.hidden = false;
        app.active = 0;
        assert!(app.accounts[0].pins.is_empty());
        assert!(app.accounts[1].pins.contains_key("19:c"));
        assert!(!app.accounts[0].chats.iter().any(|c| c.id == "19:c"));
        assert!(app.accounts[1].pending_hide.is_empty());
    }
}
