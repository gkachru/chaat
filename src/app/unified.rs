//! Unified Chats and the account rail: which view shows, per-account actions,
//! and the look (name, icon, colour) of each account.
use super::*;
use crate::account_look::{self, Look};
use crate::settings::AccountLook;

impl App {
    /// Shows Unified Chats and remembers it for the next start.
    pub(crate) fn apply_show_unified(&mut self) {
        self.unified = true;
        self.settings.unified = true;
        self.mark_settings_dirty();
        self.fetch_view(Trigger::Opened);
        self.fan_out_view(Trigger::Opened);
    }

    /// Leaves Unified Chats for one account. An index out of range is ignored.
    pub(crate) fn apply_show_account(&mut self, index: usize) {
        if index >= self.accounts.len() || self.settings.unified_only {
            return;
        }
        self.unified = false;
        self.settings.unified = false;
        self.mark_settings_dirty();
        self.apply_switch_account(index);
    }

    /// Keeps Unified Chats on with no way to leave it, or gives the switch back.
    pub(crate) fn apply_set_unified_only(&mut self, only: bool) {
        self.settings.unified_only = only;
        self.mark_settings_dirty();
        if only {
            self.apply_show_unified();
        }
    }

    /// Opens a chat in account `index` and marks every other copy read.
    pub(crate) fn apply_open_shared_chat(&mut self, index: usize, id: String, ctx: &egui::Context) {
        if index >= self.accounts.len() {
            return;
        }
        if self.active != index {
            self.apply_switch_account(index);
        }
        self.open_conversation(ConversationId::Chat(id.clone()));
        for other in self.chat_accounts(&id) {
            if other != index {
                self.apply_for_account(other, Action::MarkChatRead(id.clone()), ctx);
            }
        }
    }

    /// Indexes of the accounts whose chat list holds `chat_id`.
    pub fn chat_accounts(&self, chat_id: &str) -> Vec<usize> {
        (0..self.accounts.len())
            .filter(|&i| self.accounts[i].chats.iter().any(|c| c.id == chat_id))
            .collect()
    }

    /// Sets the availability. In Unified Chats it goes to every account that
    /// may write presence; the others are named in one toast.
    pub(crate) fn apply_set_availability_all(&mut self, availability: Option<Availability>) {
        if !self.unified_view() {
            self.apply_set_availability(availability);
            return;
        }
        let mut skipped = Vec::new();
        for index in 0..self.accounts.len() {
            if index == self.active {
                self.apply_set_availability(availability);
            } else if self.in_account(index, |app| {
                app.has_scope(crate::auth::PRESENCE_WRITE_SCOPE)
            }) {
                self.in_account(index, |app| {
                    app.hidden = false;
                    app.apply_set_availability(availability);
                });
            } else {
                skipped.push(self.account_look(index).name);
            }
        }
        if !skipped.is_empty() {
            self.toast(
                ToastKind::Info,
                format!(
                    "Status not changed for {}: open that account and set it once to allow it.",
                    skipped.join(", ")
                ),
            );
        }
    }

    /// Runs an action as if the account at `index` were active; the active
    /// account is restored afterwards.
    pub(crate) fn apply_for_account(&mut self, index: usize, action: Action, ctx: &egui::Context) {
        if index >= self.accounts.len() {
            return;
        }
        // The person did this, so the account is not a background one: its
        // own handlers (mark read, send) must not skip as they do for events.
        // The urgent-repeat stop and toasts of a handler therefore apply to
        // this account's own state, never to the visible one, and toasts
        // carry no account prefix (the person just acted on that account).
        // `in_account` restores `active` and `hidden`.
        self.in_account(index, |app| {
            app.hidden = false;
            app.apply(action, ctx)
        });
    }

    /// Stores the look under the account's id. Blank names count as unset.
    pub(crate) fn apply_set_account_look(&mut self, index: usize, mut look: AccountLook) {
        let Some(id) = self.accounts.get(index).and_then(|a| a.id.clone()) else {
            return;
        };
        look.name = look
            .name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty());
        if look == AccountLook::default() {
            self.settings.account_looks.remove(&id);
        } else {
            self.settings.account_looks.insert(id, look);
        }
        self.mark_settings_dirty();
    }

    /// One fetch of the section list per background account that has an id;
    /// the active account fetches its own view in the caller. Only the list
    /// is asked for: the other accounts' open conversations are not on
    /// screen. Called from actions only, never from drawing.
    pub(crate) fn fan_out_view(&mut self, trigger: Trigger) {
        for index in 0..self.accounts.len() {
            if index != self.active && self.accounts[index].id.is_some() {
                self.in_account(index, |app| app.fetch_section(trigger));
            }
        }
    }

    /// Name, initials, icon and colour of the account at `index`.
    pub(crate) fn account_look(&self, index: usize) -> Look {
        let usernames: Vec<String> = self.accounts.iter().map(|a| a.username()).collect();
        let custom: Vec<Option<String>> = self
            .accounts
            .iter()
            .map(|a| {
                a.id.as_ref()
                    .and_then(|id| self.settings.account_looks.get(id))
                    .and_then(|l| l.name.clone())
            })
            .collect();
        let name = account_look::names(&usernames, &custom)
            .into_iter()
            .nth(index)
            .unwrap_or_default();
        let id = self
            .accounts
            .get(index)
            .and_then(|a| a.id.clone())
            .unwrap_or_default();
        account_look::resolve(
            &id,
            name,
            self.settings.account_looks.get(&id),
            self.palette.dark,
        )
    }

    /// Unified Chats shows only with two or more accounts; the remembered
    /// choice in `settings.unified` applies again once a second one exists.
    pub fn unified_view(&self) -> bool {
        (self.unified || self.settings.unified_only) && self.accounts.len() > 1
    }

    /// Unread chats over every account.
    pub(crate) fn total_unread(&self) -> usize {
        self.accounts.iter().map(|a| a.unread_chats()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::demo_app;

    #[test]
    fn select_meeting_sets_active_selection() {
        let (mut app, _d) = demo_app();
        let ctx = egui::Context::default();
        app.apply(Action::SelectMeeting("m1".into()), &ctx);
        assert_eq!(app.selected_meeting.as_deref(), Some("m1"));
    }

    #[test]
    fn open_shared_chat_marks_every_copy_read() {
        let (mut app, _d) = demo_app();
        let ctx = egui::Context::default();
        let me1 = app.accounts[1].me().to_string();
        let id = app.accounts[1]
            .chats
            .iter()
            .find(|c| c.is_unread(&me1))
            .map(|c| c.id.clone())
            .expect("unread chat in account 1");
        let copy = app.accounts[1]
            .chats
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .clone();
        app.accounts[0].chats.push(copy);
        assert_eq!(app.chat_accounts(&id), vec![0, 1]);
        let before = app.accounts[0].open.clone();
        app.apply(Action::OpenSharedChat(1, id.clone()), &ctx);
        assert_eq!(app.active, 1);
        assert_eq!(app.open, Some(ConversationId::Chat(id.clone())));
        for i in [0, 1] {
            let me = app.accounts[i].me().to_string();
            let chat = app.accounts[i].chats.iter().find(|c| c.id == id).unwrap();
            assert!(!chat.is_unread(&me), "account {i}");
        }
        assert_eq!(app.accounts[0].open, before);
    }

    #[test]
    fn chat_accounts_lists_holders() {
        let (app, _d) = demo_app();
        let id = app.accounts[0].chats[0].id.clone();
        assert_eq!(app.chat_accounts(&id), vec![0]);
        assert!(app.chat_accounts("nope").is_empty());
    }

    #[test]
    fn availability_fans_out_in_unified() {
        let (mut app, _d) = demo_app();
        app.demo = false;
        app.accounts[0].granted = crate::auth::PRESENCE_WRITE_SCOPE.to_string();
        app.accounts[1].granted = String::new();
        app.unified = true;
        let ctx = egui::Context::default();
        app.apply(Action::SetAvailability(Some(Availability::Busy)), &ctx);
        let busy = |a: &Account| a.my_presence.as_ref().map(|p| p.availability);
        assert_eq!(busy(&app.accounts[0]), Some(Availability::Busy));
        assert_ne!(busy(&app.accounts[1]), Some(Availability::Busy));
        assert!(
            app.toasts
                .iter()
                .any(|t| t.text.starts_with("Status not changed for Fabrikam"))
        );

        app.demo = true;
        app.unified = false;
        app.active = 1;
        app.apply(Action::SetAvailability(Some(Availability::Away)), &ctx);
        assert_eq!(busy(&app.accounts[1]), Some(Availability::Away));
        assert_eq!(busy(&app.accounts[0]), Some(Availability::Busy));
    }

    #[test]
    fn unified_only_forces_unified_and_blocks_show_account() {
        let (mut app, _d) = demo_app();
        let ctx = egui::Context::default();
        app.unified = false;
        app.apply(Action::SetUnifiedOnly(true), &ctx);
        assert!(app.unified && app.settings.unified_only && app.unified_view());
        app.apply(Action::ShowAccount(1), &ctx);
        assert!(app.unified);
        assert_eq!(app.active, 0);
        app.apply(Action::SetUnifiedOnly(false), &ctx);
        assert!(!app.settings.unified_only);
    }

    #[test]
    fn show_account_leaves_unified() {
        let (mut app, _d) = demo_app();
        app.unified = true;
        app.apply_show_account(1);
        assert!(!app.unified);
        assert_eq!(app.active, 1);
        assert!(!app.settings.unified);
        app.apply_show_account(9);
        assert_eq!(app.active, 1);
    }

    #[test]
    fn unified_view_needs_two_accounts() {
        let (mut app, _d) = demo_app();
        app.unified = true;
        assert!(app.unified_view());
        app.accounts.truncate(1);
        assert!(!app.unified_view());
    }

    #[test]
    fn show_unified_sets_flag_and_saves() {
        let (mut app, _d) = demo_app();
        app.unified = false;
        app.settings_dirty = false;
        app.apply_show_unified();
        assert!(app.unified && app.settings.unified && app.settings_dirty);
    }

    #[test]
    fn for_account_restores_active() {
        let (mut app, _d) = demo_app();
        let ctx = egui::Context::default();
        let me = app.accounts[1].me().to_string();
        let id = app.accounts[1]
            .chats
            .iter()
            .find(|c| c.is_unread(&me))
            .map(|c| c.id.clone())
            .expect("demo account 1 has an unread chat");
        app.apply_for_account(1, Action::MarkChatRead(id.clone()), &ctx);
        assert_eq!(app.active, 0);
        let chat = app.accounts[1].chats.iter().find(|c| c.id == id).unwrap();
        assert!(!chat.is_unread(&me));
    }

    #[test]
    fn set_look_persists_by_id() {
        let (mut app, _d) = demo_app();
        app.settings_dirty = false;
        let look = AccountLook {
            name: Some("  Work ".into()),
            icon: Some("star".into()),
            color: None,
        };
        app.apply_set_account_look(1, look.clone());
        let id = app.accounts[1].id.clone().unwrap();
        assert_eq!(
            app.settings.account_looks[&id].name.as_deref(),
            Some("Work")
        );
        assert!(app.settings_dirty);
        app.settings_dirty = false;
        app.apply_set_account_look(9, look);
        assert!(!app.settings_dirty);
        app.apply_set_account_look(
            1,
            AccountLook {
                name: Some("  ".into()),
                ..Default::default()
            },
        );
        assert!(!app.settings.account_looks.contains_key(&id));
    }

    type Commands = tokio::sync::mpsc::UnboundedReceiver<crate::backend::Command>;

    /// Three signed-in accounts on recording backends, not the demo.
    fn recording_app() -> (App, Vec<Commands>) {
        let (mut app, first) = crate::app::tests::app();
        app.accounts[0].id = Some("u-0.t".into());
        let mut receivers = vec![first];
        for i in 1..3 {
            let (backend, rx) = Backend::recording();
            let mut account = Account::new(backend);
            account.id = Some(format!("u-{i}.t"));
            account.auth = AuthStatus::SignedIn {
                account: format!("u-{i}.t"),
                name: format!("Me {i}"),
                username: format!("me{i}@org{i}.com"),
            };
            app.accounts.push(account);
            receivers.push(rx);
        }
        (app, receivers)
    }

    /// Every list fetch an account's backend was sent since the last drain.
    fn fetches(rx: &mut Commands) -> Vec<(Resource, Trigger)> {
        let mut out = Vec::new();
        while let Ok(command) = rx.try_recv() {
            if let crate::backend::Command::Account(AccountCommand::Fetch { resource, trigger }) =
                command
            {
                out.push((resource, trigger));
            }
        }
        out
    }

    // Counted through the recording backends (`Backend::recording`), so
    // production code has no test hook.
    #[test]
    fn fan_out_asks_each_background_account_once() {
        let (mut app, mut rx) = recording_app();
        let ctx = egui::Context::default();
        let once = vec![(Resource::Chats, Trigger::Opened)];
        app.apply(Action::ShowUnified, &ctx);
        assert_eq!(app.active, 0);
        for (i, r) in rx.iter_mut().enumerate() {
            assert_eq!(fetches(r), once, "account {i}");
        }

        app.apply(Action::Navigate(Section::Chats), &ctx);
        for (i, r) in rx.iter_mut().enumerate() {
            assert_eq!(fetches(r), once, "account {i}");
        }

        // An account with no id yet is not asked.
        app.accounts[2].id = None;
        app.apply(Action::ShowUnified, &ctx);
        // Drain the active account too, or its fetch from this ShowUnified
        // would be counted below as one sent by drawing.
        assert_eq!(fetches(&mut rx[0]), once);
        assert_eq!(fetches(&mut rx[1]), once);
        assert!(fetches(&mut rx[2]).is_empty());
        app.accounts[2].id = Some("u-2.t".into());

        // Drawing never fetches for any account.
        let ctx = egui::Context::default();
        crate::ui::install(&ctx, true);
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| crate::ui::show(&mut app, ui));
            // No renderer here: drop the texture uploads on purpose.
            output.textures_delta.clear();
        }
        for (i, r) in rx.iter_mut().enumerate() {
            assert!(fetches(r).is_empty(), "drawing fetched for account {i}");
        }
    }

    #[test]
    fn mark_read_in_unified_touches_only_owner() {
        let (mut app, _d) = demo_app();
        let ctx = egui::Context::default();
        app.unified = true;
        let me0 = app.accounts[0].me().to_string();
        let me1 = app.accounts[1].me().to_string();
        let id = app.accounts[1]
            .chats
            .iter()
            .find(|c| c.is_unread(&me1) && app.chat_accounts(&c.id) == vec![1])
            .map(|c| c.id.clone())
            .expect("unread chat only in account 1");
        let state = |app: &App, i: usize, me: &str| -> Vec<(String, bool)> {
            app.accounts[i]
                .chats
                .iter()
                .map(|c| (c.id.clone(), c.is_unread(me)))
                .collect()
        };
        let before0 = state(&app, 0, &me0);
        let before1 = state(&app, 1, &me1);
        let action = Action::ForAccount(1, Box::new(Action::MarkChatRead(id.clone())));
        app.apply(action, &ctx);
        assert_eq!(app.active, 0);
        assert!(!app.hidden);
        assert!(app.unified);
        assert_eq!(state(&app, 0, &me0), before0);
        for ((cid, was), (_, now)) in before1.iter().zip(&state(&app, 1, &me1)) {
            if *cid == id {
                assert!(*was && !*now);
            } else {
                assert_eq!(was, now, "chat {cid}");
            }
        }
    }

    #[test]
    fn account_look_names_from_usernames() {
        let (app, _d) = demo_app();
        assert_eq!(app.account_look(0).name, "Contoso");
        assert_eq!(app.account_look(1).name, "Fabrikam");
        assert_eq!(
            app.total_unread(),
            app.accounts[0].unread_chats() + app.accounts[1].unread_chats()
        );
    }
}
