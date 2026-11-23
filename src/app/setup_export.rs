//! The setup file export form.

use std::path::PathBuf;

use super::App;
use crate::backend::ServiceCommand;
use crate::setup_file::SetupPayload;

/// The export form's state.
#[derive(Clone, Debug, Default)]
pub struct ExportForm {
    pub open: bool,
    pub org_label: String,
    pub include_relay: bool,
    pub import_by: String,
    /// The generated passphrase, shown once.
    pub passphrase: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
}
impl App {
    /// Opens or closes the export form.
    pub(super) fn open_export_form(&mut self, open: bool) {
        self.export = if open {
            ExportForm {
                open: true,
                org_label: self.settings.org_label.clone().unwrap_or_default(),
                ..Default::default()
            }
        } else {
            ExportForm::default()
        };
    }

    /// Checks the form, asks where to save, and starts the export.
    pub(super) fn export_setup(&mut self) {
        if self.export.busy {
            return;
        }
        let Some(import_by) = self.validate_export() else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Create a setup file")
            .set_file_name("Chaat setup.chaatsetup")
            .add_filter("Chaat setup file", &["chaatsetup"])
            .save_file()
        else {
            return;
        };
        self.send_export(path, import_by);
    }

    /// Hides the passphrase shown after an export.
    pub(super) fn dismiss_export_passphrase(&mut self) {
        self.export.passphrase = None;
        self.export.open = false;
    }

    /// Checks the form and returns the import-by date, when any. Sets
    /// `error` and returns `None` on a problem.
    fn validate_export(&mut self) -> Option<Option<String>> {
        let label = self.export.org_label.trim();
        if label.is_empty() || label.chars().count() > 80 || label.chars().any(char::is_control) {
            self.export.error = Some("Enter an organization name of up to 80 characters.".into());
            return None;
        }
        let by = self.export.import_by.trim();
        let import_by = if by.is_empty() {
            None
        } else {
            let today = jiff::Zoned::now().date();
            match by.parse::<jiff::civil::Date>() {
                Ok(date) if date >= today && by.len() == 10 => Some(by.to_owned()),
                _ => {
                    self.export.error = Some("Enter a date as YYYY-MM-DD, today or later.".into());
                    return None;
                }
            }
        };
        self.export.error = None;
        Some(import_by)
    }

    /// Builds the payload and hands it to the service backend.
    fn send_export(&mut self, path: PathBuf, import_by: Option<String>) {
        let include_relay = self.export.include_relay
            && self.settings.relay_url.is_some()
            && self.settings.relay_key_set;
        let payload = SetupPayload {
            format: 1,
            created: jiff::Timestamp::now().to_string(),
            org_label: self.export.org_label.trim().to_owned(),
            client_id: self.settings.client_id.clone(),
            tenant: self.settings.tenant.clone(),
            relay_url: if include_relay {
                self.settings.relay_url.clone()
            } else {
                None
            },
            // The service backend reads the key from the credential store.
            relay_key: None,
            import_by,
        };
        self.export.busy = true;
        self.service.send(ServiceCommand::ExportSetup {
            path,
            payload,
            include_relay_key: include_relay,
        });
    }

    /// The service finished a `Command::ExportSetup`.
    pub(super) fn on_setup_exported(&mut self, result: Result<String, String>) {
        self.export.busy = false;
        match result {
            Ok(passphrase) => {
                self.export.error = None;
                self.export.passphrase = Some(passphrase);
            }
            Err(message) => self.export.error = Some(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Command, ServiceEvent, Waker};
    use crate::model::SetupAction;
    use crate::paths::AppDirs;
    use crate::settings::Settings;

    fn app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
        let (backend, _) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-setup-export-unused");
        let mut app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            true,
        );
        let (service, commands) = Backend::recording();
        app.service = service;
        app.apply_setup_action(SetupAction::OpenExportForm(true));
        (app, commands)
    }

    #[test]
    fn empty_label_rejected() {
        let (mut app, _) = app();
        app.export.org_label = "   ".into();
        assert!(app.validate_export().is_none());
        assert!(app.export.error.is_some());
        app.export.org_label = "x".repeat(81);
        assert!(app.validate_export().is_none());
        app.export.org_label = "Contoso".into();
        assert_eq!(app.validate_export(), Some(None));
        assert!(app.export.error.is_none());
    }

    #[test]
    fn past_date_rejected() {
        let (mut app, _) = app();
        app.export.org_label = "Contoso".into();
        app.export.import_by = "2020-01-01".into();
        assert!(app.validate_export().is_none());
        assert!(app.export.error.is_some());
        app.export.import_by = "soon".into();
        assert!(app.validate_export().is_none());
        let today = jiff::Zoned::now().date().to_string();
        app.export.import_by = today.clone();
        assert_eq!(app.validate_export(), Some(Some(today)));
    }

    #[test]
    fn send_export_includes_relay_only_with_key() {
        let (mut app, mut commands) = app();
        app.export.org_label = " Contoso ".into();
        app.export.include_relay = true;
        app.settings.relay_url = Some("https://relay.example.com".into());
        app.send_export(PathBuf::from("a.chaatsetup"), None);
        assert!(app.export.busy);
        match commands.try_recv() {
            Ok(Command::Service(ServiceCommand::ExportSetup {
                payload,
                include_relay_key,
                ..
            })) => {
                assert!(!include_relay_key, "no stored key");
                assert!(payload.relay_url.is_none());
                assert_eq!(payload.org_label, "Contoso");
                assert_eq!(payload.format, 1);
            }
            _ => panic!("expected ExportSetup"),
        }
        app.settings.relay_key_set = true;
        app.send_export(PathBuf::from("a.chaatsetup"), None);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::Service(ServiceCommand::ExportSetup {
                include_relay_key: true,
                ..
            }))
        ));
    }

    #[test]
    fn passphrase_shown_then_dismissed() {
        let (mut app, _) = app();
        app.export.busy = true;
        app.on_setup_event(ServiceEvent::SetupExported(Ok("a-b-c-d-e-f".into())));
        assert!(!app.export.busy);
        assert_eq!(app.export.passphrase.as_deref(), Some("a-b-c-d-e-f"));
        app.apply_setup_action(SetupAction::DismissPassphrase);
        assert!(app.export.passphrase.is_none());
        app.export.busy = true;
        app.on_setup_event(ServiceEvent::SetupExported(Err("nope".into())));
        assert_eq!(app.export.error.as_deref(), Some("nope"));
        assert!(!app.export.busy);
    }
}
