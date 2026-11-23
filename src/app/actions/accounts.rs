//! App actions: accounts.

use super::super::*;

impl App {
    pub(in crate::app) fn apply_sign_in(&mut self) {
        if matches!(self.auth, AuthStatus::Expired { .. }) {
            self.reauth = Some(self.auth.clone());
        }
        self.backend.send(AccountCommand::SignIn {
            extra_scopes: Vec::new(),
            device_code: false,
        });
    }

    pub(in crate::app) fn apply_cancel_sign_in(&mut self) {
        self.backend.send(AccountCommand::CancelSignIn);
        self.auth = self.reauth.take().unwrap_or(AuthStatus::SignedOut);
    }

    pub(in crate::app) fn apply_request_scope(&mut self, scope: &'static str) {
        self.overlay = None;
        if !self.demo {
            self.backend.send(AccountCommand::SignIn {
                extra_scopes: vec![scope.to_owned()],
                device_code: false,
            });
            self.toast(
                ToastKind::Info,
                "Approve the permission in your browser, then try again.",
            );
        }
    }

    pub(in crate::app) fn apply_save_app_registration(
        &mut self,
        client_id: String,
        tenant: String,
    ) {
        let client_id = client_id.trim().to_owned();
        if !crate::settings::is_guid(&client_id) {
            self.toast(
                ToastKind::Error,
                "The application (client) ID is a GUID like 00000000-0000-0000-0000-000000000000.",
            );
            return;
        }
        self.onboarding_forced = false;
        self.settings.client_id = client_id;
        self.settings.tenant = match tenant.trim() {
            "" => "organizations".into(),
            other => other.to_owned(),
        };
        self.mark_settings_dirty();
        self.save_settings();
        if !self.demo {
            for index in 0..self.accounts.len() {
                self.start_account(index);
            }
        }
    }

    pub(in crate::app) fn apply_switch_account(&mut self, index: usize) {
        if index < self.accounts.len() && index != self.active {
            self.active = index;
            self.overlay = None;
            self.switcher = Switcher::default();
            self.scroll_to_end = 4;
            self.remember_accounts();
            self.fetch_view(Trigger::Focused);
            self.fetch(Resource::MyPresence, Trigger::Focused);
            if self.settings_page == Some(SettingsPage::OutOfOffice) {
                self.show_out_of_office();
            }
        }
    }

    pub(in crate::app) fn apply_add_account(&mut self) {
        if self.demo {
            self.toast(
                ToastKind::Info,
                "The demo has its accounts already: switch between them in this menu.",
            );
            return;
        }
        self.overlay = None;
        let from = self.active;
        let index = self.add_slot(None);
        self.adding_from = Some(from);
        self.active = index;
        if self.settings.has_app_registration() {
            // Straight to Microsoft's account picker.
            self.backend.send(AccountCommand::SignIn {
                extra_scopes: Vec::new(),
                device_code: false,
            });
        }
    }

    pub(in crate::app) fn apply_cancel_add_account(&mut self) {
        if let Some(from) = self.adding_from.take() {
            self.backend.send(AccountCommand::CancelSignIn);
            let adding = self.active;
            self.retire_slot(adding);
            self.active = if from > adding { from - 1 } else { from }
                .min(self.accounts.len().saturating_sub(1));
        }
    }

    pub(in crate::app) fn sign_out(&mut self) {
        self.overlay = None;
        if self.demo {
            return;
        }
        // Signing out of Chaat also ends plugin sign-ins for that account.
        if let Some(id) = self.id.clone() {
            self.forget_account_in_plugins(&id);
        }
        self.sign_out_browser();
        self.backend.send(AccountCommand::SignOut);
        let index = self.active;
        self.retire_slot(index);
        if self.accounts.is_empty() {
            self.add_slot(None);
            self.active = 0;
        }
        self.remember_accounts();
        self.fetch_view(Trigger::Focused);
    }
}
