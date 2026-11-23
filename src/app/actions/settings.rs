//! App actions: settings.

use super::super::*;

impl App {
    pub(in crate::app) fn apply_download_update(&mut self) {
        if let crate::updates::Status::Available {
            release,
            reason: None,
        } = &self.update
        {
            self.service
                .send(ServiceCommand::DownloadUpdate(release.clone()));
            self.update = crate::updates::Status::Downloading {
                received: 0,
                total: 0,
            };
        }
    }

    pub(in crate::app) fn apply_install_update(&mut self) {
        let unsent = self.accounts.iter().any(|account| {
            !account.pending_sends.is_empty()
                || account.drafts.iter().any(|(conversation, draft)| {
                    // Text the backend has saved survives a restart.
                    !draft.files.is_empty()
                        || !draft.images.is_empty()
                        || draft.pending_pastes > 0
                        || (draft.editing.is_some() && !draft.text.is_empty())
                        || !draft.is_saved(account.saved_drafts.get(conversation))
                })
        });
        if unsent {
            self.toast(
                ToastKind::Info,
                "Send or clear your drafts and finish pending sends before restarting to update.",
            );
        } else if matches!(self.update, crate::updates::Status::Ready(_)) {
            let crate::updates::Status::Ready(prepared) =
                std::mem::replace(&mut self.update, crate::updates::Status::Installing)
            else {
                unreachable!()
            };
            self.service.send(ServiceCommand::InstallUpdate(prepared));
        }
    }

    pub(in crate::app) fn apply_set_availability(&mut self, availability: Option<Availability>) {
        if !self.ensure_scope(crate::auth::PRESENCE_WRITE_SCOPE) {
            return;
        }
        // Shown at once; Graph's answer replaces it.
        let shown = Presence {
            availability: availability.unwrap_or(Availability::Available),
            activity: None,
            status_message: self
                .my_presence
                .as_ref()
                .and_then(|p| p.status_message.clone()),
        };
        let me = self.me().to_owned();
        self.presence.insert(me, shown.clone());
        self.my_presence = Some(shown);
        self.sync_plugin_presence();
        self.send_account(AccountCommand::SetAvailability(availability));
    }

    pub(in crate::app) fn apply_open_status_message(&mut self) {
        if !self.ensure_scope(crate::auth::PRESENCE_WRITE_SCOPE) {
            return;
        }
        self.form_status = self
            .my_presence
            .as_ref()
            .and_then(|p| p.status_message.clone())
            .unwrap_or_default();
        self.overlay = Some(Overlay::StatusMessage);
    }

    pub(in crate::app) fn apply_set_status_message(&mut self, text: String) {
        self.overlay = None;
        let text = text.trim().to_owned();
        let mut presence = self.my_presence.clone().unwrap_or_default();
        presence.status_message = (!text.is_empty()).then(|| text.clone());
        let me = self.me().to_owned();
        self.presence.insert(me, presence.clone());
        self.my_presence = Some(presence);
        self.sync_plugin_presence();
        if !self.demo {
            let expires = self.status_expiry_millis();
            self.backend
                .send(AccountCommand::SetStatusMessage { text, expires });
        }
    }

    pub(in crate::app) fn apply_open_out_of_office(&mut self, ctx: &egui::Context) {
        self.apply(Action::ShowSettingsPage(SettingsPage::OutOfOffice), ctx);
        self.ensure_scope(crate::auth::MAILBOX_SETTINGS_SCOPE);
    }

    pub(in crate::app) fn apply_set_out_of_office(&mut self, value: crate::model::OutOfOffice) {
        if self.demo {
            self.form_out_of_office =
                crate::model::OutOfOfficeForm::new(&value, crate::time::now());
            self.out_of_office = Some(value);
        } else if !self.out_of_office_saving {
            self.out_of_office_saving = true;
            self.backend.send(AccountCommand::SetOutOfOffice(value));
        }
    }

    pub(in crate::app) fn apply_set_presence_session(&mut self, on: bool) {
        self.settings.presence_session = on;
        self.mark_settings_dirty();
        for account in &self.accounts {
            account.backend.send(AccountCommand::PresenceSession(on));
        }
    }

    pub(in crate::app) fn apply_show_settings_page(&mut self, page: SettingsPage) {
        self.settings_page = Some(page);
        self.last_settings_page = page;
        self.overlay = None;
        if page == SettingsPage::OutOfOffice {
            self.show_out_of_office();
        }
    }

    pub(in crate::app) fn apply_set_keep_running(&mut self, on: bool) {
        self.settings.keep_running = on;
        self.mark_settings_dirty();
        // The tray is the way back to a closed window: make it now.
        if on && self.tray.is_none() && !self.demo {
            self.tray = crate::platform::tray(&self.waker);
            if let Some(tray) = &mut self.tray {
                tray.attach();
            }
        }
    }

    pub(in crate::app) fn apply_set_custom_titlebar(&mut self, on: bool, ctx: &egui::Context) {
        self.settings.custom_titlebar = on;
        self.mark_settings_dirty();
        fastframe_titlebar::set_enabled(on);
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(
            !fastframe_titlebar::enabled(),
        ));
    }

    pub(in crate::app) fn apply_set_relay(&mut self, url: String) {
        let url = url.trim().to_owned();
        if !url.is_empty() && crate::realtime::normalize_relay(&url).is_none() {
            self.toast(ToastKind::Error, "The relay must be an https:// address.");
            return;
        }
        self.settings.relay_url = (!url.is_empty()).then_some(url);
        self.mark_settings_dirty();
        self.send_relay();
        // A manual relay wins; clearing it brings the plugin back.
        self.sync_push_route(false);
    }

    pub(in crate::app) fn apply_set_relay_key(&mut self, key: Option<String>) {
        let key = key.map(|key| key.trim().to_owned());
        if key
            .as_deref()
            .is_some_and(|key| !crate::realtime::valid_relay_key(key))
        {
            self.toast(
                ToastKind::Error,
                "The access key must be at least 16 characters, without spaces.",
            );
            return;
        }
        // The service backend owns the credential store write, so
        // accounts never write the shared entry at the same time.
        if !self.demo {
            self.service.send(ServiceCommand::SaveRelayKey(
                key.map(crate::realtime::RelayKey),
            ));
        }
    }

    pub(in crate::app) fn apply_look_up_names(&mut self, users: Vec<String>) {
        // Asked once each; the demo has no directory to ask.
        let demo = self.demo;
        for user in users {
            if self.person_name(&user).is_none() && self.names_asked.insert(user.clone()) && !demo {
                self.backend.send(AccountCommand::UserName(user));
            }
        }
    }

    pub(in crate::app) fn apply_retry_relay(&mut self) {
        // Accounts already connected keep their connection.
        if !self.demo {
            for account in &self.accounts {
                if matches!(account.push, crate::realtime::PushStatus::Failed(_)) {
                    account
                        .backend
                        .send(AccountCommand::Relay(self.settings.relay_url.clone()));
                }
            }
        }
    }

    pub(in crate::app) fn apply_toggle_perf_overlay(&mut self) {
        self.settings.perf_overlay = !self.settings.perf_overlay;
        self.mark_settings_dirty();
    }

    pub(in crate::app) fn apply_open_log_folder(&mut self) {
        // A path, not a file:// link: OpenUrl opens web links only.
        // The error is logged by kind; its text can hold the path.
        if let Err(error) = open::that_detached(&self.dirs.state) {
            log::warn!("could not open the log folder: {:?}", error.kind());
            self.toast(ToastKind::Error, "Chaat could not open the log folder.");
        }
    }

    pub(in crate::app) fn apply_refresh(&mut self) {
        self.fetch_view(Trigger::Refresh);
        if self.demo {
            self.toast(ToastKind::Info, "The demo has no account to refresh from.");
        }
    }

    pub(in crate::app) fn check_update(&mut self, download: bool) {
        if self.update.busy() || matches!(self.update, crate::updates::Status::Ready(_)) {
            return;
        }
        self.update = crate::updates::Status::Checking;
        self.settings.last_update_check = crate::time::now();
        self.mark_settings_dirty();
        self.service.send(ServiceCommand::CheckUpdate { download });
    }
}
