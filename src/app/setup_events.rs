//! Backend events of the setup file flow.

use super::{App, SetupStage};
use crate::backend::{AuthStatus, Event};
use crate::model::ToastKind;

const NOT_A_SETUP_FILE: &str = "This isn't a Chaat setup file.";
const KEY_NOT_SAVED: &str =
    "The access key could not be saved. Add it in Settings \u{203a} Push updates.";

impl App {
    /// Applies a `SetupOpened`, `SetupApplied` or `SetupExported` event.
    /// Answers for an older token are ignored.
    pub fn on_setup_event(&mut self, event: Event) {
        match event {
            Event::SetupOpened { token, result } => {
                if token != self.setup.token {
                    return;
                }
                let SetupStage::Opening(path) = self.setup.stage.clone() else {
                    return;
                };
                self.setup.stage = match result {
                    Ok(summary) => {
                        self.setup.passphrase.clear();
                        self.setup.error = None;
                        SetupStage::Review(summary)
                    }
                    Err(message) if message == NOT_A_SETUP_FILE => SetupStage::Failed(message),
                    Err(message) => {
                        self.setup.error = Some(message);
                        SetupStage::Passphrase(path)
                    }
                };
            }
            Event::SetupApplied { token, ok } => {
                if token != self.setup.token || self.setup.stage != SetupStage::Applying {
                    return;
                }
                self.setup.stage = SetupStage::Idle;
                // The settings are saved either way: the forced welcome
                // screen has done its job.
                self.onboarding_forced = false;
                if ok {
                    if self.setup.key_included {
                        self.settings.relay_key_set = true;
                        self.mark_settings_dirty();
                        self.send_relay();
                    }
                } else {
                    self.setup.welcome = None;
                    self.toast(ToastKind::Error, KEY_NOT_SAVED);
                }
                self.setup.key_included = false;
                self.continue_to_sign_in();
            }
            Event::SetupExported(result) => self.on_setup_exported(result),
            _ => {}
        }
    }

    /// First run: once the file is applied and the account is ready to sign
    /// in, opens Microsoft's sign-in, as "Continue to sign in" promised.
    /// The applied event and the account's restart arrive in either order,
    /// so both call this.
    pub(crate) fn continue_to_sign_in(&mut self) {
        if self.setup.sign_in_after
            && self.setup.stage == SetupStage::Idle
            && matches!(self.auth, AuthStatus::SignedOut)
        {
            self.setup.sign_in_after = false;
            self.actions.push(crate::model::Action::SignIn);
        }
    }

    /// Called when `account` signed in, to finish a pending setup.
    pub fn on_signed_in_setup(&mut self, account: &str) {
        if let Some(expected) = self.settings.expected_tenant.clone()
            && account
                .rsplit_once('.')
                .is_some_and(|(_, tid)| tid != expected)
        {
            let org = self.settings.org_label.clone().unwrap_or_default();
            self.toast(
                ToastKind::Error,
                format!("This account is from a different organization than {org}'s setup file."),
            );
        }
        if let Some(org) = self.setup.welcome.take() {
            self.toast(ToastKind::Info, format!("Chaat is set up for {org}."));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Command, Waker};
    use crate::paths::AppDirs;
    use crate::settings::Settings;
    use crate::setup_file::SetupSummary;
    use std::path::PathBuf;

    fn app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
        let (backend, commands) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-setup-events-unused");
        let app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            true,
        );
        (app, commands)
    }

    fn summary() -> SetupSummary {
        SetupSummary {
            org_label: "Contoso".into(),
            client_id: "11111111-1111-1111-1111-111111111111".into(),
            tenant: "organizations".into(),
            expected_tenant: None,
            relay_url: None,
            relay_key_included: false,
            created: "2026-11-21".into(),
            import_by: None,
        }
    }

    #[test]
    fn stale_token_ignored() {
        let (mut app, _) = app();
        app.setup.token = 3;
        app.setup.stage = SetupStage::Opening(PathBuf::from("a.chaatsetup"));
        app.on_setup_event(Event::SetupOpened {
            token: 2,
            result: Ok(summary()),
        });
        assert!(matches!(app.setup.stage, SetupStage::Opening(_)));
        app.on_setup_event(Event::SetupOpened {
            token: 3,
            result: Err("wrong passphrase".into()),
        });
        assert_eq!(
            app.setup.stage,
            SetupStage::Passphrase(PathBuf::from("a.chaatsetup"))
        );
        assert_eq!(app.setup.error.as_deref(), Some("wrong passphrase"));
        app.setup.stage = SetupStage::Opening(PathBuf::from("a.chaatsetup"));
        app.on_setup_event(Event::SetupOpened {
            token: 3,
            result: Err(NOT_A_SETUP_FILE.into()),
        });
        assert_eq!(app.setup.stage, SetupStage::Failed(NOT_A_SETUP_FILE.into()));
        app.setup.stage = SetupStage::Opening(PathBuf::from("a.chaatsetup"));
        app.on_setup_event(Event::SetupOpened {
            token: 3,
            result: Ok(summary()),
        });
        assert!(matches!(app.setup.stage, SetupStage::Review(_)));
        app.setup.stage = SetupStage::Applying;
        app.on_setup_event(Event::SetupApplied { token: 2, ok: true });
        assert_eq!(app.setup.stage, SetupStage::Applying);
    }

    #[test]
    fn applied_ok_sets_relay_key_set_and_resends_relay() {
        let (mut app, mut commands) = app();
        app.demo = false;
        app.settings.relay_url = Some("https://relay.example.com".into());
        app.id = Some("o.t1".into());
        app.setup.token = 1;
        app.setup.stage = SetupStage::Applying;
        app.setup.key_included = true;
        app.on_setup_event(Event::SetupApplied { token: 1, ok: true });
        assert!(app.settings.relay_key_set);
        assert_eq!(app.setup.stage, SetupStage::Idle);
        assert!(
            matches!(commands.try_recv(), Ok(Command::Relay(Some(_)))),
            "the relay goes to the accounts again"
        );
    }

    #[test]
    fn applied_fail_toasts_spec_text() {
        let (mut app, _) = app();
        app.setup.token = 1;
        app.setup.stage = SetupStage::Applying;
        app.setup.key_included = true;
        app.on_setup_event(Event::SetupApplied {
            token: 1,
            ok: false,
        });
        assert!(!app.settings.relay_key_set);
        assert_eq!(
            app.toasts.last().map(|t| t.text.as_str()),
            Some("The access key could not be saved. Add it in Settings \u{203a} Push updates.")
        );
    }

    #[test]
    fn tenant_mismatch_toast() {
        let (mut app, _) = app();
        app.settings.expected_tenant = Some("t1".into());
        app.settings.org_label = Some("Contoso".into());
        app.on_signed_in_setup("o.t1");
        assert!(app.toasts.is_empty());
        app.on_signed_in_setup("o.t2");
        assert_eq!(
            app.toasts.last().map(|t| t.text.as_str()),
            Some("This account is from a different organization than Contoso's setup file.")
        );
    }

    #[test]
    fn set_up_toast_once() {
        let (mut app, _) = app();
        app.setup.welcome = Some("Contoso".into());
        app.on_signed_in_setup("o.t1");
        assert_eq!(app.toasts.len(), 1);
        assert_eq!(app.toasts[0].text, "Chaat is set up for Contoso.");
        app.toasts.clear();
        app.on_signed_in_setup("o.t1");
        assert!(app.toasts.is_empty());
    }

    #[test]
    fn first_run_apply_goes_on_to_sign_in_once() {
        let (mut app, _) = app();
        app.setup.token = 5;
        app.setup.stage = SetupStage::Applying;
        app.setup.sign_in_after = true;
        // The account is still restoring: wait for it.
        app.auth = AuthStatus::Restoring;
        app.on_setup_event(Event::SetupApplied { token: 5, ok: true });
        assert!(!app.actions.contains(&crate::model::Action::SignIn));
        // Ready to sign in: sign-in starts, once.
        app.auth = AuthStatus::SignedOut;
        app.continue_to_sign_in();
        app.continue_to_sign_in();
        let starts = app
            .actions
            .iter()
            .filter(|action| **action == crate::model::Action::SignIn)
            .count();
        assert_eq!(starts, 1);
    }

    #[test]
    fn opened_file_clears_the_kept_passphrase() {
        let (mut app, _) = app();
        app.setup.token = 2;
        app.setup.passphrase = "apple banana".into();
        app.setup.stage = SetupStage::Opening(PathBuf::from("a.chaatsetup"));
        app.on_setup_event(Event::SetupOpened {
            token: 2,
            result: Err("wrong".into()),
        });
        assert_eq!(app.setup.passphrase, "apple banana");
        app.setup.stage = SetupStage::Opening(PathBuf::from("a.chaatsetup"));
        app.on_setup_event(Event::SetupOpened {
            token: 2,
            result: Ok(summary()),
        });
        assert!(app.setup.passphrase.is_empty());
    }
}
