//! App events: session.

use super::super::*;

impl App {
    pub(in crate::app) fn handle_update(&mut self, event: crate::updates::Event) {
        match event {
            crate::updates::Event::Status(status) => {
                if matches!(status, crate::updates::Status::Ready(_)) {
                    self.toast(
                        ToastKind::Info,
                        "An update is downloaded and ready to install when you restart.",
                    );
                }
                self.update = status;
            }
            crate::updates::Event::Quit => self.quit_requested = true,
        }
    }

    pub(in crate::app) fn handle_granted(&mut self, granted: String) {
        let had = self.has_scope(crate::auth::MAILBOX_SETTINGS_SCOPE);
        self.granted = granted;
        if !self.hidden
            && !had
            && self.settings_page == Some(SettingsPage::OutOfOffice)
            && self.has_scope(crate::auth::MAILBOX_SETTINGS_SCOPE)
        {
            self.fetch(Resource::OutOfOffice, Trigger::Opened);
        }
    }

    pub(in crate::app) fn handle_out_of_office(&mut self, value: crate::model::OutOfOffice) {
        // The fields follow Graph unless the person changed them.
        let follow = self.out_of_office.as_ref().is_none_or(|old| {
            !self.form_out_of_office.changed_from(old)
                || !self.form_out_of_office.changed_from(&value)
        });
        if follow && !self.hidden {
            self.form_out_of_office =
                crate::model::OutOfOfficeForm::new(&value, crate::time::now());
        }
        self.out_of_office = Some(value);
        self.out_of_office_saving = false;
        self.no_mailbox = false;
    }

    pub(in crate::app) fn on_auth(&mut self, status: AuthStatus) {
        log::info!("account: {}", status.log_label());
        let became_signed_in = matches!(status, AuthStatus::SignedIn { .. })
            && !matches!(self.auth, AuthStatus::SignedIn { .. });
        match &status {
            AuthStatus::SignedIn { account, .. } => {
                // The same account signed in twice (through "Add account"):
                // keep the slot that had it and show that one.
                if let Some(existing) =
                    self.accounts.iter().enumerate().position(|(index, a)| {
                        index != self.active && a.id.as_deref() == Some(account)
                    })
                {
                    self.retire.push(self.active);
                    self.adding_from = None;
                    self.toast(ToastKind::Info, "That account is already signed in.");
                    self.actions
                        .push(Action::SwitchAccount(if existing > self.active {
                            existing - 1
                        } else {
                            existing
                        }));
                    return;
                }
                self.id = Some(account.clone());
                self.reauth = None;
                self.remember_browser_profile();
                if !self.hidden {
                    self.adding_from = None;
                }
            }
            AuthStatus::SignedOut => {
                self.clear_data();
                // The account has no presence now; plugins hear it.
                self.sync_plugin_presence();
            }
            AuthStatus::Failed(reason) => {
                // Signing in again did not finish: back to the cached
                // workspace, with the reason.
                if let Some(AuthStatus::Expired { username, .. }) = self.reauth.take() {
                    self.auth = AuthStatus::Expired {
                        username,
                        reason: reason.clone(),
                    };
                    return;
                }
            }
            AuthStatus::Expired { username, reason } => {
                // Keep the name the account had when Graph does not repeat it.
                if username.is_empty()
                    && let AuthStatus::SignedIn {
                        username: known, ..
                    } = &self.auth
                {
                    let known = known.clone();
                    let reason = reason.clone();
                    self.auth = AuthStatus::Expired {
                        username: known,
                        reason,
                    };
                    return;
                }
            }
            _ => {}
        }
        self.auth = status;
        if !self.hidden {
            self.continue_to_sign_in();
        }
        if became_signed_in {
            self.on_signed_in_setup();
            self.remember_accounts_for_slot();
            for resource in [
                Resource::Profile,
                Resource::Chats,
                Resource::Teams,
                Resource::Calendar,
                Resource::MyPresence,
            ] {
                self.fetch(resource, Trigger::Start);
            }
            if let Some(open) = &self.open {
                self.fetch(Resource::Messages(open.clone()), Trigger::Start);
            }
        }
    }

    /// `remember_accounts` from inside event handling, where the active
    /// index may point at a hidden account.
    pub(in crate::app) fn remember_accounts_for_slot(&mut self) {
        let ids: Vec<String> = self.accounts.iter().filter_map(|a| a.id.clone()).collect();
        if self.settings.accounts != ids {
            self.settings.accounts = ids;
            self.mark_settings_dirty();
        }
        if !self.hidden && self.settings.account != self.id {
            self.settings.account = self.id.clone();
            self.mark_settings_dirty();
        }
        self.sync_plugin_accounts();
    }
}
