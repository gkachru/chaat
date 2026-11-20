//! Process-wide settings, as JSON in the config folder.
//!
//! Unknown fields are ignored and missing ones take their defaults, so files
//! written by older or newer versions keep loading. Writes go through a
//! temporary file and a rename. Secrets never go here: refresh tokens live in
//! the OS credential store (`crate::secrets`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow the desktop's light or dark preference.
    #[default]
    System,
    Light,
    Dark,
    /// A shared fastframe palette by file name (`Nord.json`, ...).
    Palette(String),
}

/// One external plugin the person added (Settings › Plugins).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginEntry {
    /// A stable name made by `plugin_id_for`; keys commands, events and the
    /// plugin's secret option values.
    pub id: String,
    /// The plugin's executable.
    pub path: PathBuf,
    /// Whether Chaat launches it.
    pub enabled: bool,
    /// What its `ready` asked for (`accounts`, `presence`), remembered so the
    /// next `hello` can answer before `ready` arrives.
    pub needs: Vec<String>,
    /// Accounts (`oid.tid`) signed out of Chaat while the plugin wasn't
    /// running: it forgets their sign-ins when it next starts.
    pub forget: Vec<String>,
    /// Values of the options the plugin described, by option ID. Secret
    /// options never appear here.
    pub options: BTreeMap<String, serde_json::Value>,
    /// IDs of the secret options that have a value in the credential store.
    pub secrets: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The Entra ID application (client) ID Chaat signs in with. Each person or
    /// organisation registers their own; see docs/azure-app-registration.md.
    pub client_id: String,
    /// `organizations` (any work or school account), or one tenant's ID or
    /// domain.
    pub tenant: String,
    pub theme: ThemeChoice,
    pub notifications: bool,
    /// Keep syncing in the tray when the window closes.
    pub keep_running: bool,
    /// The interface zoom.
    pub zoom: f32,
    pub sidebar_width: f32,
    /// The account shown at start (kept for settings files from 0.1).
    pub account: Option<String>,
    /// Every signed-in account, in the order they were added.
    pub accounts: Vec<String>,
    /// Keep a presence session while Chaat runs, so people see you available
    /// (needs Presence.ReadWrite).
    pub presence_session: bool,
    pub perf_overlay: bool,
    /// Windows and Linux: draw the title bar's buttons in Chaat's own header
    /// instead of the system's window frame.
    pub custom_titlebar: bool,
    pub window: Option<WindowGeometry>,
    /// A chaat-relay for push updates, `https://…`.
    pub relay_url: Option<String>,
    /// Whether the relay's access key is in the credential store. The key
    /// itself never goes in this file.
    pub relay_key_set: bool,
    pub calls: CallSettings,
    /// External plugins, in the order they were added.
    pub plugins: Vec<PluginEntry>,
    /// Set once the call-alerts fields in `calls` were carried into `plugins`,
    /// so removing every plugin later does not bring the old one back.
    pub plugins_migrated: bool,
    /// Check GitHub once a day and download verified app updates.
    pub automatic_updates: bool,
    pub last_update_check: i64,
}

/// How calls and meetings open (docs/calling.md).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CallSettings {
    pub open_in: crate::calling::CallsOpenIn,
    /// Edge or Chrome for app windows; `None` takes the first installed.
    pub browser: Option<crate::calling::browser::BrowserKind>,
    /// Move Chaat beside the call window while a call runs.
    pub dock: bool,
    /// One browser profile per account, so each profile holds one Microsoft
    /// sign-in. Profile folders by account ID.
    pub profile_per_account: bool,
    pub profiles: std::collections::BTreeMap<String, String>,
    /// Signing out of Chaat also signs the call browser out of Microsoft.
    pub sign_out_browser: bool,
    /// Every web link (messages, calendar, "Open in Teams") opens in the
    /// call browser and profile instead of the default browser.
    pub links_in_browser: bool,
    /// An external plugin that delivers incoming-call alerts (Chaat itself uses
    /// only official Graph APIs, which cannot ring; see docs/calling.md). When
    /// set and `call_alerts` is on, Chaat launches this executable and shows a
    /// ring toast for each call it reports. Empty means no plugin.
    pub call_alerts_plugin: Option<std::path::PathBuf>,
    /// Whether the call-alerts plugin is active. Off unless a path is set.
    pub call_alerts: bool,
    /// Accounts (`oid.tid`) signed out of Chaat while the call-alerts plugin
    /// wasn't running: it forgets their sign-ins when it next starts.
    pub call_alerts_forget: Vec<String>,
}

impl Default for CallSettings {
    fn default() -> Self {
        Self {
            open_in: crate::calling::CallsOpenIn::AppWindow,
            browser: None,
            dock: true,
            profile_per_account: false,
            profiles: std::collections::BTreeMap::new(),
            sign_out_browser: false,
            links_in_browser: false,
            call_alerts_plugin: None,
            call_alerts: false,
            call_alerts_forget: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            tenant: "organizations".into(),
            theme: ThemeChoice::System,
            notifications: true,
            // Off by default: without push notifications a hidden Chaat
            // cannot learn about new messages (docs/realtime.md).
            keep_running: false,
            zoom: 1.0,
            sidebar_width: 300.0,
            account: None,
            accounts: Vec::new(),
            presence_session: true,
            perf_overlay: false,
            custom_titlebar: true,
            window: None,
            relay_url: None,
            relay_key_set: false,
            calls: CallSettings::default(),
            plugins: Vec::new(),
            plugins_migrated: false,
            automatic_updates: true,
            last_update_check: 0,
        }
    }
}

impl Settings {
    /// Reads the settings, falling back to defaults when the file is missing
    /// or damaged (the damaged copy is kept beside it for inspection).
    pub fn load(path: &Path) -> Self {
        let mut settings = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(settings) => settings,
                Err(error) => {
                    log::warn!("settings unreadable ({error}); starting from defaults");
                    let _ = std::fs::rename(path, path.with_extension("json.damaged"));
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        };
        // Environment overrides, for development and managed installs.
        if let Some(client_id) = std::env::var("CHAAT_CLIENT_ID").ok()
            && !client_id.trim().is_empty()
        {
            settings.client_id = client_id.trim().to_owned();
        }
        if let Some(tenant) = std::env::var("CHAAT_TENANT").ok()
            && !tenant.trim().is_empty()
        {
            settings.tenant = tenant.trim().to_owned();
        }
        // 0.1 remembered one account; carry it over.
        if settings.accounts.is_empty()
            && let Some(account) = &settings.account
        {
            settings.accounts.push(account.clone());
        }
        settings.migrate_plugins();
        settings.zoom = settings.zoom.clamp(0.5, 3.0);
        settings.sidebar_width = settings.sidebar_width.clamp(220.0, 520.0);
        settings
    }

    /// Carries the old call-alerts fields into `plugins`, once. Returns whether
    /// anything changed. The old fields stay as they are.
    pub fn migrate_plugins(&mut self) -> bool {
        if self.plugins_migrated {
            return false;
        }
        self.plugins_migrated = true;
        if let Some(path) = self.calls.call_alerts_plugin.clone() {
            self.plugins.push(PluginEntry {
                id: "call-alerts".into(),
                path,
                enabled: self.calls.call_alerts,
                needs: vec!["accounts".into()],
                forget: self.calls.call_alerts_forget.clone(),
                ..PluginEntry::default()
            });
        }
        true
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        crate::paths::write_atomically(path, &json)
    }

    pub fn has_app_registration(&self) -> bool {
        is_guid(self.client_id.trim())
    }
}

/// An ID for a plugin from its file name: lowercase ASCII letters and digits
/// joined by single dashes, made unique among `taken` with `-2`, `-3`, ...
pub fn plugin_id_for(path: &Path, taken: &[String]) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut base = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c);
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    let base = base.trim_end_matches('-');
    let base = if base.is_empty() { "plugin" } else { base };
    let mut id = base.to_owned();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Whether `value` looks like a GUID (Entra application and tenant IDs).
pub fn is_guid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults_and_unknown_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"notifications": false, "from_the_future": 1}"#).unwrap();
        let settings = Settings::load(&path);
        assert!(!settings.notifications);
        assert_eq!(settings.tenant, "organizations");
    }

    #[test]
    fn damaged_files_are_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{not json").unwrap();
        let settings = Settings::load(&path);
        assert_eq!(settings, Settings::load(&dir.path().join("absent.json")));
        assert!(path.with_extension("json.damaged").exists());
    }

    #[test]
    fn save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            client_id: "00000000-0000-0000-0000-000000000000".into(),
            theme: ThemeChoice::Palette("Nord.json".into()),
            plugins_migrated: true,
            ..Default::default()
        };
        settings.save(&path).unwrap();
        // Environment overrides would make this flaky if set in CI.
        if std::env::var("CHAAT_CLIENT_ID").ok().is_none()
            && std::env::var("CHAAT_TENANT").ok().is_none()
        {
            assert_eq!(Settings::load(&path), settings);
        }
    }

    #[test]
    fn a_single_account_from_0_1_is_carried_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"account": "o.t"}"#).unwrap();
        assert_eq!(Settings::load(&path).accounts, ["o.t"]);
    }

    const OLD: &str = r#"{"calls":{"call_alerts_plugin":"C:/p/cca.exe","call_alerts":true,"call_alerts_forget":["o.t"]}}"#;

    #[test]
    fn old_call_alerts_fields_migrate_to_one_entry() {
        let mut settings: Settings = serde_json::from_str(OLD).unwrap();
        assert!(settings.migrate_plugins());
        assert_eq!(
            settings.plugins,
            [PluginEntry {
                id: "call-alerts".into(),
                path: "C:/p/cca.exe".into(),
                enabled: true,
                needs: vec!["accounts".into()],
                forget: vec!["o.t".into()],
                ..PluginEntry::default()
            }]
        );
        assert!(settings.calls.call_alerts_plugin.is_some());
    }

    #[test]
    fn migration_runs_once_even_after_every_plugin_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, OLD).unwrap();
        let mut settings = Settings::load(&path);
        assert_eq!(settings.plugins.len(), 1);
        assert!(!settings.migrate_plugins());
        settings.plugins.clear();
        settings.save(&path).unwrap();
        let reloaded = Settings::load(&path);
        assert!(reloaded.plugins.is_empty());
        assert!(reloaded.plugins_migrated);
    }

    #[test]
    fn no_old_plugin_means_no_entry_but_migrated() {
        let mut settings = Settings::default();
        assert!(settings.migrate_plugins());
        assert!(settings.plugins.is_empty());
        assert!(settings.plugins_migrated);
        assert!(!settings.migrate_plugins());
    }

    #[test]
    fn entries_round_trip_and_unknown_fields_are_ignored() {
        let entry = PluginEntry {
            id: "status-light".into(),
            path: "/opt/busy".into(),
            enabled: true,
            needs: vec!["presence".into()],
            forget: vec![],
            options: BTreeMap::from([("brightness".into(), serde_json::json!(3))]),
            secrets: vec!["token".into()],
        };
        let settings = Settings {
            plugins: vec![entry.clone()],
            plugins_migrated: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.plugins, [entry]);
        let partial: PluginEntry =
            serde_json::from_str(r#"{"id":"x","path":"p","from_the_future":true}"#).unwrap();
        assert_eq!(partial.id, "x");
        assert!(!partial.enabled);
        assert!(partial.options.is_empty());
    }

    #[test]
    fn ids_are_lowercase_ascii_dashes() {
        let id = |p: &str, taken: &[&str]| {
            let taken: Vec<String> = taken.iter().map(|t| t.to_string()).collect();
            plugin_id_for(Path::new(p), &taken)
        };
        assert_eq!(id("C:/x/Chaat Status-Light.exe", &[]), "chaat-status-light");
        assert_eq!(
            id("C:/x/Chaat Status-Light.exe", &["chaat-status-light"]),
            "chaat-status-light-2"
        );
        assert_eq!(id("/x/a.exe", &["a", "a-2"]), "a-3");
        assert_eq!(id("/x/__My  Plug_in__.sh", &[]), "my-plug-in");
        assert_eq!(id("/x/\u{e9}\u{e9}.exe", &[]), "plugin");
        assert_eq!(id("", &[]), "plugin");
    }

    #[test]
    fn guids() {
        assert!(is_guid("6731de76-14a6-49ae-97bc-6eba6914391e"));
        assert!(!is_guid("not-a-guid"));
        assert!(!is_guid("6731de76-14a6-49ae-97bc-6eba6914391"));
    }
}
