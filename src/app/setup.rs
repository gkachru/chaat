//! The setup file flow in the App: choose, unlock, review, apply.

use std::path::{Path, PathBuf};

use super::App;
use crate::backend::{AuthStatus, Command};
use crate::model::Action;
use crate::setup_file::SetupSummary;

/// Set to `1` to show the welcome screen at launch even when Chaat is
/// configured, for trying the onboarding. A file applied there overrides
/// only the settings it carries.
pub const ONBOARDING_ENV: &str = "CHAAT_ONBOARDING";

/// Whether `CHAAT_ONBOARDING=1` is set.
pub fn onboarding_forced_by_env() -> bool {
    std::env::var(ONBOARDING_ENV).is_ok_and(|value| value.trim() == "1")
}

/// Where the setup file flow stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SetupStage {
    #[default]
    Idle,
    Passphrase(PathBuf),
    Opening(PathBuf),
    Review(SetupSummary),
    Applying,
    Failed(String),
}

/// The setup file flow's state.
#[derive(Clone, Debug, Default)]
pub struct SetupFlow {
    pub stage: SetupStage,
    /// Show the manual sign-in fields as well.
    pub manual: bool,
    pub passphrase: String,
    pub token: u64,
    /// Why the last open failed, shown under the passphrase field.
    pub error: Option<String>,
    /// The applied file carried a relay access key.
    pub key_included: bool,
    /// Organization to greet once after sign-in.
    pub welcome: Option<String>,
    /// First run: start sign-in once the applied registration is ready.
    pub sign_in_after: bool,
}

impl App {
    /// Runs one setup flow action.
    pub fn setup_action(&mut self, action: Action) {
        match action {
            Action::ChooseSetupFile => {
                if self.setup_busy() {
                    return;
                }
                let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose a setup file")
                    .add_filter("Chaat setup file", &["chaatsetup"])
                    .pick_file()
                else {
                    return;
                };
                self.setup_picked(path);
            }
            Action::DroppedSetupFile(path) => {
                if self.setup_busy() {
                    return;
                }
                self.setup_picked(path);
            }
            Action::OpenSetupFile => {
                let SetupStage::Passphrase(path) = self.setup.stage.clone() else {
                    return;
                };
                self.setup.token += 1;
                self.setup.error = None;
                // Kept until the file opens, so a typo can be corrected
                // instead of typing six words again.
                let passphrase = crate::setup_file::passphrase::normalize(&self.setup.passphrase);
                self.service.send(Command::OpenSetupFile {
                    path: path.clone(),
                    passphrase,
                    token: self.setup.token,
                });
                self.setup.stage = SetupStage::Opening(path);
            }
            Action::ApplySetup => {
                let SetupStage::Review(summary) = self.setup.stage.clone() else {
                    return;
                };
                self.setup.key_included = summary.relay_key_included;
                self.setup.welcome = Some(summary.org_label.clone());
                // Nobody has signed in yet: go straight on to sign-in.
                self.setup.sign_in_after = self.settings.accounts.is_empty()
                    && !self
                        .accounts
                        .iter()
                        .any(|account| matches!(account.auth, AuthStatus::SignedIn { .. }));
                // Only a new registration signs accounts in again.
                let registration_changed = summary.client_id != self.settings.client_id
                    || summary.tenant != self.settings.tenant;
                self.settings.client_id = summary.client_id;
                self.settings.tenant = summary.tenant;
                // A file without a relay leaves the person's own relay alone.
                if summary.relay_url.is_some() {
                    self.settings.relay_url = summary.relay_url;
                }
                self.settings.org_label = Some(summary.org_label);
                self.settings.expected_tenant = summary.expected_tenant;
                self.mark_settings_dirty();
                self.save_settings();
                self.service.send(Command::ApplySetup {
                    token: self.setup.token,
                });
                self.setup.stage = SetupStage::Applying;
                if !self.demo && registration_changed {
                    for index in 0..self.accounts.len() {
                        self.start_account(index);
                    }
                }
            }
            Action::CancelSetup => {
                self.discard_setup();
                self.setup.stage = SetupStage::Idle;
                self.setup.passphrase.clear();
                self.setup.error = None;
            }
            Action::ShowSetupManual(show) => self.setup.manual = show,
            Action::SkipOnboarding => {
                self.discard_setup();
                self.setup.stage = SetupStage::Idle;
                self.setup.passphrase.clear();
                self.setup.error = None;
                self.onboarding_forced = false;
            }
            _ => {}
        }
    }

    /// A file is being opened or applied; new files wait.
    fn setup_busy(&self) -> bool {
        matches!(
            self.setup.stage,
            SetupStage::Opening(_) | SetupStage::Applying
        )
    }

    /// Takes a chosen or dropped file to the passphrase stage.
    fn setup_picked(&mut self, path: PathBuf) {
        self.discard_setup();
        self.setup.passphrase.clear();
        self.setup.error = None;
        self.setup.stage = if is_setup_file(&path) {
            SetupStage::Passphrase(path)
        } else {
            SetupStage::Failed("Only a .chaatsetup file can be imported here".into())
        };
    }

    /// Tells the service to drop an opened file it still holds, and makes
    /// any late answer for it stale.
    fn discard_setup(&mut self) {
        if matches!(
            self.setup.stage,
            SetupStage::Opening(_) | SetupStage::Review(_)
        ) {
            self.service.send(Command::DiscardSetup {
                token: self.setup.token,
            });
            self.setup.token += 1;
        }
    }
}

/// Whether `path` looks like a setup file by its extension.
pub fn is_setup_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("chaatsetup"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Waker};
    use crate::paths::AppDirs;
    use crate::settings::Settings;

    fn app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
        let (backend, _) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-setup-unused");
        let mut app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            true,
        );
        let (service, commands) = Backend::recording();
        app.service = service;
        (app, commands)
    }

    fn summary() -> SetupSummary {
        SetupSummary {
            org_label: "Contoso".into(),
            client_id: "11111111-1111-1111-1111-111111111111".into(),
            tenant: "22222222-2222-2222-2222-222222222222".into(),
            expected_tenant: Some("22222222-2222-2222-2222-222222222222".into()),
            relay_url: Some("https://relay.example.com".into()),
            relay_key_included: true,
            created: "2026-11-21".into(),
            import_by: None,
        }
    }

    #[test]
    fn extension_check_ignores_case() {
        assert!(is_setup_file(Path::new("a/T.CHAATSETUP")));
        assert!(is_setup_file(Path::new("x.chaatsetup")));
        assert!(!is_setup_file(Path::new("a.txt")));
        assert!(!is_setup_file(Path::new("chaatsetup")));
    }

    #[test]
    fn wrong_extension_failed() {
        let (mut app, _) = app();
        app.setup_action(Action::DroppedSetupFile("a.txt".into()));
        assert_eq!(
            app.setup.stage,
            SetupStage::Failed("Only a .chaatsetup file can be imported here".into())
        );
        app.setup_action(Action::DroppedSetupFile("a.chaatsetup".into()));
        assert_eq!(
            app.setup.stage,
            SetupStage::Passphrase(PathBuf::from("a.chaatsetup"))
        );
    }

    #[test]
    fn open_keeps_passphrase_and_bumps_token() {
        let (mut app, mut commands) = app();
        app.setup_action(Action::DroppedSetupFile("a.chaatsetup".into()));
        app.setup.passphrase = "  Apple Banana-cherry ".into();
        app.setup.error = Some("old".into());
        let before = app.setup.token;
        app.setup_action(Action::OpenSetupFile);
        assert_eq!(app.setup.token, before + 1);
        // Kept for a retry; cleared once the file opens (setup_events).
        assert_eq!(app.setup.passphrase, "  Apple Banana-cherry ");
        assert_eq!(app.setup.error, None);
        assert_eq!(
            app.setup.stage,
            SetupStage::Opening(PathBuf::from("a.chaatsetup"))
        );
        match commands.try_recv() {
            Ok(Command::OpenSetupFile {
                path,
                passphrase,
                token,
            }) => {
                assert_eq!(path, PathBuf::from("a.chaatsetup"));
                assert_eq!(passphrase, "apple-banana-cherry");
                assert_eq!(token, before + 1);
            }
            _ => panic!("expected OpenSetupFile"),
        }
        // Only from the passphrase stage.
        app.setup_action(Action::OpenSetupFile);
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn cancel_sends_discard() {
        let (mut app, mut commands) = app();
        app.setup.token = 4;
        app.setup.stage = SetupStage::Review(summary());
        app.setup_action(Action::CancelSetup);
        assert_eq!(app.setup.stage, SetupStage::Idle);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::DiscardSetup { token: 4 })
        ));
        assert_eq!(app.setup.token, 5, "a late answer is stale");
        // Nothing pending: nothing to discard.
        app.setup_action(Action::CancelSetup);
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn apply_saves_settings_and_sends_apply() {
        let (mut app, mut commands) = app();
        app.setup.token = 7;
        app.setup.stage = SetupStage::Review(summary());
        app.setup_action(Action::ApplySetup);
        let s = summary();
        assert_eq!(app.settings.client_id, s.client_id);
        assert_eq!(app.settings.tenant, s.tenant);
        assert_eq!(app.settings.relay_url, s.relay_url);
        assert_eq!(app.settings.org_label.as_deref(), Some("Contoso"));
        assert_eq!(app.settings.expected_tenant, s.expected_tenant);
        assert_eq!(app.setup.stage, SetupStage::Applying);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::ApplySetup { token: 7 })
        ));
    }

    #[test]
    fn forced_welcome_hides_the_workspace_until_skipped() {
        let (mut app, _) = app();
        assert!(app.shows_workspace());
        app.onboarding_forced = true;
        assert!(!app.shows_workspace());
        app.setup.stage = SetupStage::Review(summary());
        app.setup_action(Action::SkipOnboarding);
        assert!(!app.onboarding_forced);
        assert_eq!(app.setup.stage, SetupStage::Idle);
        assert!(app.shows_workspace());
    }

    #[test]
    fn forced_welcome_ends_when_a_file_is_applied() {
        let (mut app, _) = app();
        app.onboarding_forced = true;
        app.setup.token = 3;
        app.setup.stage = SetupStage::Review(summary());
        app.setup_action(Action::ApplySetup);
        // Still forced while applying, so the progress stays on screen.
        assert!(app.onboarding_forced);
        app.on_setup_event(crate::backend::Event::SetupApplied { token: 3, ok: true });
        assert!(!app.onboarding_forced);
    }

    #[test]
    fn manual_toggles() {
        let (mut app, _) = app();
        app.setup_action(Action::ShowSetupManual(true));
        assert!(app.setup.manual);
        app.setup_action(Action::ShowSetupManual(false));
        assert!(!app.setup.manual);
    }
}
