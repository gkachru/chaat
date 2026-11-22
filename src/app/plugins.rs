//! The plugin list at run time: which plugins run, what they said about
//! themselves, the accounts and presence they were told, and their sign-in
//! cards. Plugins are process-wide, so this state lives on `App`, not on an
//! `Account`. The processes run on the service backend (`plugin::host`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::App;
use super::PushPluginState;
use crate::backend::Command;
use crate::model::{Action, PluginSignIn, SettingsPage, ToastKind};
use crate::plugin::event::PluginEvent;
use crate::plugin::options;
use crate::plugin::presence;
use crate::plugin::protocol::{
    HostMessage, OptionKind, OptionSpec, PluginAccount, PluginInfo, StatusState,
};
use crate::plugin::registry;
use crate::settings::{PluginEntry, plugin_id_for};

/// The shortest time between two toasts from one plugin.
const PLUGIN_TOAST_GAP: Duration = Duration::from_secs(10);

/// How long an action's button shows "Working…" without an answer.
pub const ACTION_WORKING_LIMIT: Duration = Duration::from_secs(30);

/// The last status of one option or action: its state, the plugin's words
/// (plain text) and when it arrived.
pub type OptionStatusEntry = (StatusState, Option<String>, Instant);

/// How long a new plugin may take to send `ready` in the add flow before
/// Chaat calls it failed.
pub const ADD_START_LIMIT: Duration = Duration::from_secs(15);

/// Which view of Settings › Plugins shows. Interface state only, never saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PluginsView {
    /// The list of plugins.
    #[default]
    List,
    /// One plugin's page (plugin id).
    Plugin(String),
    /// The add flow of a new plugin (plugin id).
    Adding(String),
}

/// How a plugin's row dot looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryLevel {
    /// Needs the person (warning).
    Attention,
    /// Stopped with an error (danger).
    Error,
    /// Working (success).
    Ok,
    /// Off, starting or not used (dim).
    Off,
}

/// One line about a plugin, in Chaat's words, for the plugins list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSummary {
    pub level: SummaryLevel,
    pub text: String,
}

impl PluginSummary {
    /// Whether the row goes in the "Needs you" group.
    pub fn needs_you(&self) -> bool {
        matches!(self.level, SummaryLevel::Attention | SummaryLevel::Error)
    }
}

/// What Chaat knows about one plugin of the plugin list while it runs.
#[derive(Debug, Default)]
pub struct PluginState {
    /// Whether the plugin last said it is running.
    pub running: bool,
    /// How the plugin describes itself (plain, shortened text).
    pub info: PluginInfo,
    /// Known capabilities its `ready` offered.
    pub offers: Vec<&'static str>,
    /// Known needs its `ready` asked for.
    pub needs: Vec<&'static str>,
    /// The options it described.
    pub options: Vec<OptionSpec>,
    /// Whether the current run sent its `ready`; no option values go to a run
    /// before it described its options.
    pub ready_seen: bool,
    /// The last status of each option or action its current `ready` describes,
    /// by id. Cleared when the plugin (re)starts.
    pub statuses: BTreeMap<String, OptionStatusEntry>,
    /// When the person last clicked each action's button, until a status
    /// (`ok` or `error`) for it arrives. See `PluginState::action_working`.
    pub action_started: BTreeMap<String, Instant>,
    /// Sign-ins it is waiting for, one per account.
    pub sign_in: Vec<PluginSignIn>,
    /// Accounts (`oid.tid`) it said it listens for.
    pub listening: Vec<String>,
    /// The last notice about it (why it stopped), for the status line.
    pub message: Option<String>,
    /// Needs its current `ready` declared that the person has neither allowed
    /// nor refused: Settings › Plugins asks about them.
    pub pending_needs: Vec<&'static str>,
    /// The hash of its file when it no longer matches the stored one. The
    /// plugin did not run; the person decides whether to run the new file.
    pub file_changed: Option<String>,
    /// The pending set already announced with a toast.
    pub(super) notified_pending: Vec<&'static str>,
    /// Accounts whose sign-in it already asked for (and raised the window),
    /// since it last started or that account signed in.
    pub(super) asked: Vec<Option<String>>,
    /// The accounts it was last told about; `None` while it needs none.
    pub(super) sent_accounts: Option<Vec<PluginAccount>>,
    /// The availability and activity it was last sent.
    pub(super) sent_presence: Option<(String, Option<String>)>,
}

impl PluginState {
    /// Forgets what belongs to one process run.
    fn stopped(&mut self) {
        self.running = false;
        self.ready_seen = false;
        self.sign_in.clear();
        self.asked.clear();
        self.listening.clear();
        self.sent_presence = None;
        self.statuses.clear();
        self.action_started.clear();
    }

    /// Whether an action's button shows "Working…": the person clicked it and
    /// no `ok` or `error` came since, or its last status is `working`; for at
    /// most 30 seconds either way.
    pub fn action_working(&self, id: &str, now: Instant) -> bool {
        let within = |since: Instant| now.saturating_duration_since(since) < ACTION_WORKING_LIMIT;
        self.action_started
            .get(id)
            .is_some_and(|since| within(*since))
            || self
                .statuses
                .get(id)
                .is_some_and(|(state, _, at)| *state == StatusState::Working && within(*at))
    }

    /// Takes one status; the latest per option always wins. The host already
    /// caps how many lines a plugin sends (`plugin::host`), and only options
    /// the current `ready` described are kept, so there is no limit here: a
    /// dropped update would leave a stale status on screen.
    fn take_status(&mut self, id: String, state: StatusState, message: Option<String>) {
        let now = Instant::now();
        // An answer ends the wait of a button that was clicked.
        if state != StatusState::Working {
            self.action_started.remove(&id);
        }
        self.statuses.insert(id, (state, message, now));
    }
}

/// A plugin entry's stored option values, as the checks in `plugin::options`
/// take them.
fn stored_map(entry: &PluginEntry) -> serde_json::Map<String, serde_json::Value> {
    entry
        .options
        .iter()
        .map(|(option, value)| (option.clone(), value.clone()))
        .collect()
}

impl App {
    /// Sends to the service backend, except in the demo.
    pub(super) fn send_service(&self, command: Command) {
        if !self.demo {
            self.service.send(command);
        }
    }

    /// Chaat's signed-in accounts, as a plugin that needs them is told about
    /// them: ids and sign-in names only.
    pub fn plugin_accounts(&self) -> Vec<PluginAccount> {
        self.accounts
            .iter()
            .filter_map(|account| {
                Some(PluginAccount {
                    id: account.id.clone()?,
                    username: account.username(),
                })
            })
            .collect()
    }

    /// The sign-in name Chaat knows for an account id.
    pub(super) fn plugin_username(&self, account: Option<&str>) -> Option<String> {
        let account = account?;
        self.accounts
            .iter()
            .find(|a| a.id.as_deref() == Some(account))
            .map(|a| a.username())
            .filter(|name| !name.is_empty())
    }

    /// What Chaat calls a plugin in its own notices: the plugin's cleaned name,
    /// else its id.
    pub fn plugin_name(&self, id: &str) -> String {
        self.plugins
            .get(id)
            .and_then(|state| state.info.name.clone())
            .unwrap_or_else(|| id.to_owned())
    }

    /// The plugin that gets rings and sign-ins for call alerts: the first
    /// enabled one in list order that is running and offers `call_alerts`.
    pub fn call_alerts_owner(&self) -> Option<String> {
        self.settings
            .plugins
            .iter()
            .filter(|entry| entry.enabled)
            .find(|entry| {
                self.plugins.get(&entry.id).is_some_and(|state| {
                    state.running && state.offers.contains(&registry::CALL_ALERTS)
                })
            })
            .map(|entry| entry.id.clone())
    }

    /// Whether the plugin declared `need` in its last `ready` and the person
    /// allowed it. Only such a need decides what Chaat sends.
    pub(super) fn plugin_need_effective(entry: &PluginEntry, need: &str) -> bool {
        entry.needs.iter().any(|n| n == need) && entry.approved_needs.iter().any(|n| n == need)
    }

    fn plugin_needs_accounts(entry: &PluginEntry) -> bool {
        Self::plugin_need_effective(entry, "accounts")
    }

    /// What a plugin receives from Chaat now, for Settings › Plugins:
    /// `accounts` (account IDs and sign-in names), `presence` (availability
    /// and activity), `push` (update channels, no account ids) and `options`
    /// (its own option values).
    pub fn plugin_receives(&self, id: &str) -> Vec<&'static str> {
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return Vec::new();
        };
        let state = self.plugins.get(id);
        let mut receives = Vec::new();
        for need in ["accounts", "presence", registry::PUSH] {
            if Self::plugin_need_effective(entry, need) {
                receives.push(need);
            }
        }
        if state.is_some_and(|state| !state.options.is_empty()) {
            receives.push("options");
        }
        receives
    }

    /// The `hello` for one plugin: accounts only if it needs them.
    fn plugin_hello(&self, entry: &PluginEntry) -> HostMessage {
        HostMessage::Hello {
            host: "chaat".into(),
            capabilities: registry::KNOWN.iter().map(|c| (*c).to_owned()).collect(),
            accounts: if Self::plugin_needs_accounts(entry) {
                self.plugin_accounts()
            } else {
                Vec::new()
            },
            // The sign-out queue is about accounts the plugin holds, which it
            // only does while it is allowed them.
            forget: if Self::plugin_needs_accounts(entry) {
                entry.forget.clone()
            } else {
                Vec::new()
            },
        }
    }

    /// Starts and stops plugin hosts to match the settings: every enabled
    /// entry with a path runs; a changed path restarts that host only.
    pub(crate) fn sync_plugins(&mut self) {
        // Without the service backend a start goes nowhere, yet would count
        // as started; `attach_service` syncs again once it is there.
        if self.demo || self.service.is_detached() {
            return;
        }
        // A plugin whose add flow is not finished runs only while its setup
        // page is open.
        let adding = match &self.plugins_view {
            PluginsView::Adding(id) => Some(id.clone()),
            _ => None,
        };
        let wanted: Vec<(String, PathBuf)> = self
            .settings
            .plugins
            .iter()
            .filter(|entry| {
                entry.enabled
                    && !entry.path.as_os_str().is_empty()
                    && (!entry.setup_pending || adding.as_deref() == Some(entry.id.as_str()))
            })
            .map(|entry| (entry.id.clone(), entry.path.clone()))
            .collect();
        let stale: Vec<String> = self
            .plugins_started
            .keys()
            .filter(|id| !wanted.iter().any(|(wanted, _)| wanted == *id))
            .cloned()
            .collect();
        for id in stale {
            self.plugins_started.remove(&id);
            self.plugin_started_at.remove(&id);
            self.retire_plugin_generation(&id);
            self.service.send(Command::StopPlugin { id: id.clone() });
            if let Some(state) = self.plugins.get_mut(&id) {
                state.stopped();
            }
            self.push_plugin_stopped(&id);
        }
        for (id, path) in wanted {
            if self.plugins_started.get(&id) == Some(&path) {
                continue;
            }
            let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
                continue;
            };
            let hello = self.plugin_hello(entry);
            let accounts = Self::plugin_needs_accounts(entry).then(|| self.plugin_accounts());
            let expected_sha256 = entry.sha256.clone();
            self.plugins_started.insert(id.clone(), path.clone());
            self.plugin_started_at.insert(id.clone(), Instant::now());
            self.push_plugin_stopped(&id);
            let state = self.plugins.entry(id.clone()).or_default();
            state.stopped();
            state.sent_accounts = accounts;
            // The host checks the file again and reports a change again.
            state.file_changed = None;
            let generation = self.next_plugin_generation(&id);
            self.service.send(Command::StartPlugin {
                id,
                path,
                hello,
                expected_sha256,
                generation,
            });
        }
        self.sync_push_route(false);
    }

    /// A generation no earlier start of any plugin had, now the current one
    /// for `id`: events from older hosts of it are dropped.
    fn next_plugin_generation(&mut self, id: &str) -> u64 {
        self.plugin_generation += 1;
        self.plugin_generations
            .insert(id.to_owned(), self.plugin_generation);
        self.plugin_generation
    }

    /// Makes the plugin's current generation one no host has, so whatever its
    /// stopped host still reports is dropped.
    pub(super) fn retire_plugin_generation(&mut self, id: &str) {
        self.next_plugin_generation(id);
    }

    /// Whether an event from a plugin's host counts: it comes from the
    /// current start, and the plugin is still in the list and turned on.
    fn plugin_event_counts(&self, id: &str, generation: u64) -> bool {
        self.plugin_generations
            .get(id)
            .is_none_or(|current| *current == generation)
            && self
                .settings
                .plugins
                .iter()
                .any(|entry| entry.id == id && entry.enabled)
    }

    /// Tells running plugins about changes to Chaat's accounts, takes
    /// returned accounts off the sign-out queues and drops state for accounts
    /// that are gone.
    pub(super) fn sync_plugin_accounts(&mut self) {
        let present: Vec<String> = self.accounts.iter().filter_map(|a| a.id.clone()).collect();
        let back: Vec<(String, Vec<String>)> = self
            .settings
            .plugins
            .iter()
            .map(|entry| {
                let back = entry
                    .forget
                    .iter()
                    .filter(|id| present.contains(id))
                    .cloned()
                    .collect::<Vec<_>>();
                (entry.id.clone(), back)
            })
            .filter(|(_, back)| !back.is_empty())
            .collect();
        for (id, back) in back {
            self.plugin_forget_done(&id, back);
        }
        if self.demo {
            return;
        }
        let accounts = self.plugin_accounts();
        let known = |account: &str| accounts.iter().any(|a| a.id == account);
        for state in self.plugins.values_mut() {
            state
                .sign_in
                .retain(|s| s.account.as_deref().is_none_or(known));
            state
                .asked
                .retain(|asked| asked.as_deref().is_none_or(known));
            state.listening.retain(|id| known(id));
        }
        let ids: Vec<String> = self
            .settings
            .plugins
            .iter()
            .filter(|entry| Self::plugin_needs_accounts(entry))
            .map(|entry| entry.id.clone())
            .filter(|id| self.plugins_started.contains_key(id))
            .collect();
        for id in ids {
            let Some(state) = self.plugins.get_mut(&id) else {
                continue;
            };
            if state.sent_accounts.as_ref() != Some(&accounts) {
                state.sent_accounts = Some(accounts.clone());
                self.send_plugin_accounts(&id, accounts.clone());
            }
        }
        self.sync_plugin_presence();
    }

    fn send_plugin_accounts(&self, id: &str, accounts: Vec<PluginAccount>) {
        self.send_service(Command::PluginSend {
            id: id.to_owned(),
            message: HostMessage::Accounts {
                accounts: accounts.clone(),
            },
        });
        self.send_service(Command::PluginHello {
            id: id.to_owned(),
            accounts: Some(accounts),
            forget: None,
        });
    }

    /// Takes accounts off one plugin's sign-out queue (forgotten by the plugin,
    /// or signed in to Chaat again) and stops resending them.
    fn plugin_forget_done(&mut self, id: &str, done: Vec<String>) {
        let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) else {
            return;
        };
        let before = entry.forget.len();
        entry.forget.retain(|account| !done.contains(account));
        if entry.forget.len() != before {
            // The restart hello carries the queue only while the plugin may
            // hold accounts.
            let forget = if Self::plugin_needs_accounts(entry) {
                entry.forget.clone()
            } else {
                Vec::new()
            };
            self.mark_settings_dirty();
            self.send_service(Command::PluginHello {
                id: id.to_owned(),
                accounts: None,
                forget: Some(forget),
            });
        }
    }

    /// Queues the signing-out account for every plugin that holds sign-ins for
    /// Chaat's accounts. A running plugin hears now; every start hears again
    /// until one confirms (`ready.forgotten`), so a quit or restart in between
    /// can't lose it.
    pub(super) fn forget_account_in_plugins(&mut self, account: &str) {
        let ids: Vec<String> = self
            .settings
            .plugins
            .iter()
            .filter(|entry| {
                Self::plugin_needs_accounts(entry) && !entry.path.as_os_str().is_empty()
            })
            .map(|entry| entry.id.clone())
            .collect();
        for id in ids {
            let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) else {
                continue;
            };
            if !entry.forget.contains(&account.to_owned()) {
                entry.forget.push(account.to_owned());
                self.mark_settings_dirty();
            }
            if self.plugins_started.contains_key(&id) {
                let forget = self
                    .settings
                    .plugins
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.forget.clone())
                    .unwrap_or_default();
                self.send_service(Command::PluginSend {
                    id: id.clone(),
                    message: HostMessage::Forget {
                        account: account.to_owned(),
                    },
                });
                self.send_service(Command::PluginHello {
                    id,
                    accounts: None,
                    forget: Some(forget),
                });
            }
        }
    }

    /// Sends each running plugin that needs presence the busiest presence
    /// across Chaat's accounts, when it differs from what it was last sent.
    pub fn sync_plugin_presence(&mut self) {
        if self.demo {
            return;
        }
        let presences: Vec<&crate::model::Presence> = self
            .accounts
            .iter()
            .filter_map(|account| account.my_presence.as_ref())
            .collect();
        let message = presence::message(presence::busiest(&presences).as_ref());
        let HostMessage::Presence {
            availability,
            activity,
        } = &message
        else {
            return;
        };
        let key = (availability.clone(), activity.clone());
        let mut to_send = Vec::new();
        for (id, state) in &mut self.plugins {
            let allowed = self
                .settings
                .plugins
                .iter()
                .any(|entry| entry.id == *id && Self::plugin_need_effective(entry, "presence"));
            if state.running
                && allowed
                && state.needs.contains(&"presence")
                && state.sent_presence.as_ref() != Some(&key)
            {
                state.sent_presence = Some(key.clone());
                to_send.push(id.clone());
            }
        }
        for id in to_send {
            self.send_service(Command::PluginSend {
                id,
                message: message.clone(),
            });
        }
    }

    /// Sends a plugin its option values, for the options its current `ready`
    /// describes only: the stored value or the default of each non-secret
    /// option, and the names of the stored secrets of its secret options (the
    /// worker adds their values).
    pub(super) fn send_plugin_options(&self, id: &str) {
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return;
        };
        let Some(state) = self.plugins.get(id) else {
            return;
        };
        if !state.ready_seen {
            return;
        }
        // Defaults filled in; secrets are named, never valued; buttons have
        // no value.
        let values: BTreeMap<String, serde_json::Value> =
            options::effective_values(&state.options, &stored_map(entry))
                .into_iter()
                .collect();
        let mut secrets = Vec::new();
        for spec in &state.options {
            if spec.kind == OptionKind::Secret
                && entry.secrets.contains(&spec.id)
                && !secrets.contains(&spec.id)
            {
                secrets.push(spec.id.clone());
            }
        }
        self.send_service(Command::PluginOptions {
            id: id.to_owned(),
            values,
            secrets,
        });
    }

    /// Raises a toast from a plugin as `Plugin “Name”: text` (`Plugin
    /// “Name” for user: text` for one account), with the name quoted so the plugin cannot pass its words off as Chaat's. A plugin
    /// gets at most one toast every 10 seconds; the rest are dropped.
    fn plugin_toast(&mut self, id: &str, kind: ToastKind, user: Option<String>, text: String) {
        let now = std::time::Instant::now();
        if self
            .plugin_toasts
            .get(id)
            .is_some_and(|last| now.duration_since(*last) < PLUGIN_TOAST_GAP)
        {
            return;
        }
        self.plugin_toasts.insert(id.to_owned(), now);
        let name = self.plugin_name(id).replace(['“', '”'], "'");
        let text = match user {
            Some(user) => format!("Plugin “{name}” for {user}: {text}"),
            None => format!("Plugin “{name}”: {text}"),
        };
        self.toast(kind, text);
    }

    /// Whether an event naming this account (if any) may count: a plugin
    /// cannot speak for an account Chaat does not have.
    fn plugin_account_counts(&self, account: Option<&str>) -> bool {
        account.is_none_or(|account| {
            self.accounts
                .iter()
                .any(|a| a.id.as_deref() == Some(account))
        })
    }

    /// Whether an event about sign-ins and rings from this plugin counts: a
    /// plugin offering call alerts counts only while it is the owner.
    fn plugin_counts(&self, id: &str) -> bool {
        let offers = self
            .plugins
            .get(id)
            .is_some_and(|state| state.offers.contains(&registry::CALL_ALERTS));
        !offers || self.call_alerts_owner().as_deref() == Some(id)
    }

    /// Applies what a plugin did.
    pub(super) fn on_plugin_event(&mut self, id: String, generation: u64, event: PluginEvent) {
        if !self.plugin_event_counts(&id, generation) {
            return;
        }
        match event {
            PluginEvent::Ready {
                info,
                offers,
                needs,
                options,
                forgotten,
            } => self.on_plugin_ready(&id, info, offers, needs, options, forgotten),
            PluginEvent::Status { running, message } => {
                let state = self.plugins.entry(id.clone()).or_default();
                state.running = running;
                if !running {
                    state.stopped();
                    self.push_plugin_stopped(&id);
                }
                self.sync_push_route(false);
                let state = self.plugins.entry(id.clone()).or_default();
                state.message = message.clone();
                if let Some(message) = message {
                    self.plugin_toast(&id, ToastKind::Error, None, message);
                }
            }
            PluginEvent::SignIn {
                account,
                verification_uri,
                user_code,
            } => {
                if !self.plugin_counts(&id) || !self.plugin_account_counts(account.as_deref()) {
                    return;
                }
                let username = self.plugin_username(account.as_deref());
                let state = self.plugins.entry(id.clone()).or_default();
                state.listening.retain(|a| Some(a) != account.as_ref());
                // Bring Chaat forward for an account's first code only; later
                // codes (an expired or declined one) update the panel quietly.
                if !state.asked.contains(&account) {
                    state.asked.push(account.clone());
                    self.wants_show = true;
                }
                let sign_in = PluginSignIn {
                    plugin: id.clone(),
                    username,
                    account,
                    verification_uri,
                    user_code,
                };
                let state = self.plugins.entry(id).or_default();
                match state
                    .sign_in
                    .iter_mut()
                    .find(|s| s.account == sign_in.account)
                {
                    Some(shown) => *shown = sign_in,
                    None => state.sign_in.push(sign_in),
                }
            }
            PluginEvent::SignedIn { account } => {
                if !self.plugin_counts(&id) || !self.plugin_account_counts(account.as_deref()) {
                    return;
                }
                let state = self.plugins.entry(id.clone()).or_default();
                state.sign_in.retain(|s| s.account != account);
                state.asked.retain(|asked| *asked != account);
                let text = match self.plugin_username(account.as_deref()) {
                    Some(user) if self.accounts.len() > 1 => {
                        format!("signed in for {user}.")
                    }
                    _ => "signed in.".to_owned(),
                };
                self.plugin_toast(&id, ToastKind::Info, None, text);
            }
            PluginEvent::Listening { account } => {
                if !self.plugin_counts(&id) || !self.plugin_account_counts(Some(&account)) {
                    return;
                }
                let state = self.plugins.entry(id).or_default();
                if !state.listening.contains(&account) {
                    state.listening.push(account);
                }
            }
            PluginEvent::Error { account, message } => {
                if !self.plugin_counts(&id) || !self.plugin_account_counts(account.as_deref()) {
                    return;
                }
                if let Some(account) = &account {
                    let state = self.plugins.entry(id.clone()).or_default();
                    state.listening.retain(|a| a != account);
                }
                let user = self.plugin_username(account.as_deref());
                self.plugin_toast(&id, ToastKind::Error, user, message);
            }
            PluginEvent::IncomingCall(alert) => {
                if self.call_alerts_owner().as_deref() == Some(id.as_str())
                    && self.plugin_account_counts(alert.account.as_deref())
                {
                    self.on_incoming_call(alert);
                }
            }
            // Trust on first use: the first hash becomes the entry's pin.
            PluginEvent::Fingerprint { sha256 } => {
                if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id)
                    && entry.sha256.is_none()
                {
                    entry.sha256 = Some(sha256);
                    self.mark_settings_dirty();
                }
            }
            // The host did not run the changed file: the person decides.
            PluginEvent::FileChanged { sha256 } => {
                let state = self.plugins.entry(id).or_default();
                state.stopped();
                state.file_changed = Some(sha256);
            }
            PluginEvent::OptionStatus {
                id: option,
                state,
                message,
            } => {
                let Some(plugin) = self.plugins.get_mut(&id) else {
                    return;
                };
                // Only the options of the current run's `ready` have a status.
                if plugin.ready_seen && plugin.options.iter().any(|spec| spec.id == option) {
                    plugin.take_status(option, state, message);
                }
            }
            PluginEvent::OptionsUpdate {
                id: option,
                choices,
            } => {
                let Some(plugin) = self.plugins.get_mut(&id) else {
                    return;
                };
                // A stored value that is no longer offered stays; the page
                // shows it as not available.
                if plugin.ready_seen
                    && let Some(spec) = plugin
                        .options
                        .iter_mut()
                        .find(|spec| spec.id == option && spec.kind == OptionKind::Choice)
                {
                    spec.choices = choices;
                }
            }
            PluginEvent::Push(event) => self.on_push_event(&id, generation, event),
        }
    }

    fn on_plugin_ready(
        &mut self,
        id: &str,
        info: PluginInfo,
        offers: Vec<&'static str>,
        needs: Vec<&'static str>,
        options: Vec<OptionSpec>,
        forgotten: Vec<String>,
    ) {
        // Only a need that is declared now and allowed counts.
        let (approved, denied) = self
            .settings
            .plugins
            .iter()
            .find(|e| e.id == id)
            .map(|e| (e.approved_needs.clone(), e.denied_needs.clone()))
            .unwrap_or_default();
        let wants_accounts =
            needs.contains(&"accounts") && approved.iter().any(|n| n == "accounts");
        let pending: Vec<&'static str> = needs
            .iter()
            .copied()
            .filter(|need| !approved.iter().any(|n| n == need) && !denied.iter().any(|n| n == need))
            .collect();
        let state = self.plugins.entry(id.to_owned()).or_default();
        state.running = true;
        state.message = None;
        state.info = info;
        state.offers = offers;
        state.needs = needs.clone();
        state.pending_needs = pending.clone();
        let announce = !pending.is_empty() && state.notified_pending != pending;
        state.notified_pending = pending;
        let dropped_accounts = !wants_accounts && state.sent_accounts.take().is_some();
        if announce {
            let name = self.plugin_name(id).replace(['“', '”'], "'");
            self.toast(
                ToastKind::Info,
                format!("Plugin “{name}” asks for access. Open Settings › Plugins to allow it."),
            );
        }
        if dropped_accounts {
            // A later `ready` that needs them again must be sent them, and a
            // crash-restart must not hand them to a plugin that stopped asking.
            self.send_service(Command::PluginHello {
                id: id.to_owned(),
                accounts: Some(Vec::new()),
                forget: Some(Vec::new()),
            });
        }
        // A stored plain value for an option that is now secret goes: secrets
        // live in the credential store only.
        let now_secret: Vec<String> = options
            .iter()
            .filter(|spec| spec.kind == OptionKind::Secret)
            .map(|spec| spec.id.clone())
            .collect();
        if let Some(state) = self.plugins.get_mut(id) {
            state.options = options;
            state.ready_seen = true;
        }
        if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) {
            let before = entry.options.len();
            let specs = self
                .plugins
                .get(id)
                .map(|state| state.options.as_slice())
                .unwrap_or_default();
            // Values that no longer fit what the plugin now describes go:
            // secrets, and anything its kind, range or choices refuse. Values
            // for options it does not describe, or of a kind this Chaat does
            // not know, are kept.
            entry.options.retain(|option, value| {
                if now_secret.contains(option) {
                    return false;
                }
                match specs.iter().find(|spec| spec.id == *option) {
                    // A choice the plugin no longer lists stays (the page shows
                    // it as not available until `options_update` offers it),
                    // if it is still a well-formed choice value.
                    Some(spec) if spec.kind == OptionKind::Choice => value
                        .as_str()
                        .is_some_and(|text| (1..=64).contains(&text.chars().count())),
                    Some(spec) if !matches!(spec.kind, OptionKind::Unsupported { .. }) => {
                        options::check_value(spec, value).is_ok_and(|checked| checked == *value)
                    }
                    _ => true,
                }
            });
            if entry.options.len() != before {
                self.mark_settings_dirty();
            }
        }
        if let Some(state) = self.plugins.get_mut(id) {
            // What a status or a click said about an option that is gone.
            let described: Vec<String> = state.options.iter().map(|s| s.id.clone()).collect();
            state
                .statuses
                .retain(|option, _| described.contains(option));
            state
                .action_started
                .retain(|option, _| described.contains(option));
        }
        let described: Vec<String> = self
            .plugins
            .get(id)
            .map(|state| state.options.iter().map(|s| s.id.clone()).collect())
            .unwrap_or_default();
        self.plugin_option_errors
            .retain(|(plugin, option), _| plugin != id || described.contains(option));
        let Some(state) = self.plugins.get_mut(id) else {
            return;
        };
        // A restarted process has not been sent the presence yet.
        state.sent_presence = None;
        let first_accounts = wants_accounts && state.sent_accounts.is_none();
        let needs: Vec<String> = needs.iter().map(|need| (*need).to_owned()).collect();
        if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id)
            && entry.needs != needs
        {
            entry.needs = needs;
            self.mark_settings_dirty();
        }
        if first_accounts {
            let accounts = self.plugin_accounts();
            if let Some(state) = self.plugins.get_mut(id) {
                state.sent_accounts = Some(accounts.clone());
            }
            self.send_plugin_accounts(id, accounts);
        }
        self.send_plugin_options(id);
        // Only what the plugin says it cleared leaves the queue.
        self.plugin_forget_done(id, forgotten);
        self.sync_plugin_presence();
        self.sync_push_route(false);
    }

    /// The ids of the required options of a plugin that have no value yet
    /// (saved secrets count as set), in the order it described them. Empty
    /// while the plugin has not described its options.
    pub fn plugin_missing_required(&self, id: &str) -> Vec<String> {
        let (Some(state), Some(entry)) = (
            self.plugins.get(id),
            self.settings.plugins.iter().find(|e| e.id == id),
        ) else {
            return Vec::new();
        };
        options::missing_required(&state.options, &stored_map(entry), &entry.secrets)
    }

    /// Adds a plugin to the list, turned on, and starts it. It stays
    /// `setup_pending` until the person finishes its add flow.
    pub(super) fn add_plugin(&mut self, path: PathBuf) {
        let taken: Vec<String> = self.settings.plugins.iter().map(|e| e.id.clone()).collect();
        let id = plugin_id_for(&path, &taken);
        self.settings.plugins.push(PluginEntry {
            id: id.clone(),
            path,
            enabled: true,
            instance: PluginEntry::new_instance(),
            setup_pending: true,
            ..PluginEntry::default()
        });
        self.mark_settings_dirty();
        self.reordering = false;
        self.plugins_view = PluginsView::Adding(id);
        self.sync_plugins();
    }

    /// Picks a plugin with the system's file picker (modal, like a dialog).
    pub(super) fn choose_plugin(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose a plugin")
            .pick_file()
        else {
            return;
        };
        self.add_plugin(path);
    }

    /// Stops a plugin and removes it, its stored secrets and its state. A
    /// running plugin that holds account sign-ins is told to forget them
    /// first.
    pub(super) fn remove_plugin(&mut self, id: &str) {
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id).cloned() else {
            return;
        };
        if self.plugins_started.contains_key(id) && Self::plugin_needs_accounts(&entry) {
            for account in self.plugin_accounts() {
                self.send_service(Command::PluginSend {
                    id: id.to_owned(),
                    message: HostMessage::Forget {
                        account: account.id,
                    },
                });
            }
        }
        // Every secret option the plugin described may have a stored value,
        // recorded or not (a save can still be in flight), so each goes.
        let mut secrets = entry.secrets.clone();
        if let Some(state) = self.plugins.get(id) {
            for spec in &state.options {
                if spec.kind == OptionKind::Secret && !secrets.contains(&spec.id) {
                    secrets.push(spec.id.clone());
                }
            }
        }
        for option in secrets {
            self.send_service(Command::SavePluginSecret {
                plugin: id.to_owned(),
                instance: entry.instance.clone(),
                option,
                value: None,
            });
        }
        self.plugin_toasts.remove(id);
        self.plugin_started_at.remove(id);
        self.plugin_option_errors
            .retain(|(plugin, _), _| plugin != id);
        if matches!(&self.plugins_view, PluginsView::Plugin(v) | PluginsView::Adding(v) if v == id)
        {
            self.plugins_view = PluginsView::List;
        }
        if self.plugins_started.remove(id).is_some() {
            self.retire_plugin_generation(id);
            self.send_service(Command::StopPlugin { id: id.to_owned() });
        }
        self.settings.plugins.retain(|e| e.id != id);
        self.plugins.remove(id);
        self.push_plugin_stopped(id);
        self.sync_push_route(false);
        self.mark_settings_dirty();
    }

    /// Starts a plugin of an unfinished add flow again. The old run is
    /// stopped and its generation retired first, so a late `ready` from it
    /// does not count for the new run, and the wait starts over. Plugins that
    /// are not in an add flow are left alone.
    fn retry_plugin_start(&mut self, id: &str) {
        if !self
            .settings
            .plugins
            .iter()
            .any(|e| e.id == id && e.setup_pending)
        {
            return;
        }
        if self.plugins_started.remove(id).is_some() {
            self.retire_plugin_generation(id);
            self.send_service(Command::StopPlugin { id: id.to_owned() });
        }
        self.plugin_started_at.remove(id);
        if let Some(state) = self.plugins.get_mut(id) {
            state.message = None;
        }
        self.sync_plugins();
    }

    /// Runs the plugin actions.
    pub(super) fn apply_plugin_action(&mut self, action: Action) {
        match action {
            Action::AddPlugin => self.choose_plugin(),
            Action::RemovePlugin(id) => self.remove_plugin(&id),
            Action::SetPluginEnabled(id, enabled) => {
                if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) {
                    entry.enabled = enabled;
                    self.mark_settings_dirty();
                    self.sync_plugins();
                }
            }
            Action::SetPluginOption {
                plugin,
                option,
                value,
            } => {
                self.set_plugin_option(plugin, option, value);
            }
            Action::RunPluginAction { plugin, action } => self.run_plugin_action(plugin, action),
            Action::ResetPluginSection { plugin, section } => {
                self.reset_plugin_section(&plugin, section.as_deref());
            }
            Action::OpenPlugin(id) => self.open_plugin(id),
            Action::BackToPlugins => {
                self.plugins_view = PluginsView::List;
                self.sync_plugins();
            }
            Action::StartReorder => {
                if self.settings.plugins.len() >= 2 {
                    self.reordering = true;
                    self.plugins_view = PluginsView::List;
                    self.sync_plugins();
                }
            }
            Action::EndReorder => self.reordering = false,
            Action::MovePlugin { id, to } => self.move_plugin(&id, to),
            Action::FinishPluginSetup { id, allow } => self.finish_plugin_setup(&id, &allow),
            Action::CancelPluginSetup(id) => {
                if self
                    .settings
                    .plugins
                    .iter()
                    .any(|e| e.id == id && e.setup_pending)
                {
                    self.remove_plugin(&id);
                }
            }
            Action::RetryPluginStart(id) => self.retry_plugin_start(&id),
            Action::SetPluginSecret {
                plugin,
                option,
                value,
            } => self.set_plugin_secret(plugin, option, value),
            Action::ApprovePluginNeeds(id) => self.decide_plugin_needs(&id, true),
            Action::DenyPluginNeeds(id) => self.decide_plugin_needs(&id, false),
            Action::TrustPluginFile(id) => self.trust_plugin_file(&id),
            Action::KeepPluginOff(id) => {
                if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) {
                    entry.enabled = false;
                    self.mark_settings_dirty();
                }
                if let Some(state) = self.plugins.get_mut(&id) {
                    state.file_changed = None;
                }
                self.sync_plugins();
            }
            Action::PluginSignIn { plugin, account } => self.send_service(Command::PluginSend {
                id: plugin,
                message: HostMessage::SignIn { account },
            }),
            Action::HidePluginSignIn { plugin, account } => {
                if let Some(state) = self.plugins.get_mut(&plugin) {
                    state.sign_in.retain(|s| s.account != account);
                }
            }
            _ => {}
        }
    }

    /// Checks and stores one option value, then sends the options. An invalid
    /// value is neither stored nor sent: its error waits in
    /// `plugin_option_errors` for the page. Secrets and buttons have no stored
    /// value, and an option the plugin does not describe is ignored.
    fn set_plugin_option(&mut self, plugin: String, option: String, value: serde_json::Value) {
        let Some(spec) = self
            .plugins
            .get(&plugin)
            .and_then(|state| state.options.iter().find(|spec| spec.id == option))
        else {
            return;
        };
        let checked = if spec.kind == OptionKind::Secret {
            Err("Secrets are saved separately.".to_owned())
        } else {
            options::check_value(spec, &value)
        };
        let key = (plugin.clone(), option.clone());
        let checked = match checked {
            Ok(checked) => checked,
            Err(error) => {
                self.plugin_option_errors.insert(key, error);
                return;
            }
        };
        self.plugin_option_errors.remove(&key);
        if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == plugin) {
            entry.options.insert(option, checked);
            self.mark_settings_dirty();
            self.send_plugin_options(&plugin);
        }
    }

    /// Checks and saves (or, with `None`, clears) a secret option value. Only
    /// a secret option the plugin described takes one; the value is trimmed
    /// and must be non-empty plain text. A refused value is not sent: its
    /// error waits in `plugin_option_errors` for the page.
    fn set_plugin_secret(&mut self, plugin: String, option: String, value: Option<String>) {
        let Some(spec) = self
            .plugins
            .get(&plugin)
            .and_then(|state| state.options.iter().find(|spec| spec.id == option))
        else {
            return;
        };
        let key = (plugin.clone(), option.clone());
        let is_secret = spec.kind == OptionKind::Secret;
        let value = match value {
            None if is_secret => None,
            None => return,
            Some(raw) => {
                let checked = if is_secret {
                    options::check_value(spec, &serde_json::Value::String(raw)).and_then(
                        |v| match v.as_str().unwrap_or_default() {
                            "" => Err("Enter a value.".to_owned()),
                            text => Ok(text.to_owned()),
                        },
                    )
                } else {
                    Err("This setting is not a secret.".to_owned())
                };
                match checked {
                    Ok(text) => Some(text),
                    Err(error) => {
                        self.plugin_option_errors.insert(key, error);
                        return;
                    }
                }
            }
        };
        self.plugin_option_errors.remove(&key);
        let instance = self
            .settings
            .plugins
            .iter()
            .find(|e| e.id == plugin)
            .map(|e| e.instance.clone());
        if let Some(instance) = instance {
            self.send_service(Command::SavePluginSecret {
                plugin,
                instance,
                option,
                value,
            });
        }
    }

    /// Tells a plugin its button was clicked: only a running plugin that has
    /// said `ready`, and only for a button it described.
    fn run_plugin_action(&mut self, plugin: String, action: String) {
        let Some(state) = self.plugins.get_mut(&plugin) else {
            return;
        };
        let is_button = state
            .options
            .iter()
            .any(|spec| spec.id == action && spec.kind == OptionKind::Action);
        if !(state.running && state.ready_seen && is_button) {
            return;
        }
        // The last answer is old now; the button shows its wait.
        state.statuses.remove(&action);
        state.action_started.insert(action.clone(), Instant::now());
        self.send_service(Command::PluginSend {
            id: plugin,
            message: HostMessage::Action { id: action },
        });
    }

    /// "Reset to defaults" for one section (`None` is the options with no
    /// section): options with a default get it, those without lose their
    /// stored value, secrets stay.
    fn reset_plugin_section(&mut self, plugin: &str, section: Option<&str>) {
        let Some(state) = self.plugins.get(plugin) else {
            return;
        };
        let defaults = options::defaults_for_section(&state.options, section);
        let cleared: Vec<String> = state
            .options
            .iter()
            .filter(|spec| spec.section.as_deref() == section)
            .filter(|spec| !matches!(spec.kind, OptionKind::Secret | OptionKind::Action))
            .map(|spec| spec.id.clone())
            .collect();
        let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == plugin) else {
            return;
        };
        for option in &cleared {
            entry.options.remove(option);
        }
        entry.options.extend(defaults);
        self.plugin_option_errors
            .retain(|(p, option), _| p != plugin || !cleared.contains(option));
        self.mark_settings_dirty();
        self.send_plugin_options(plugin);
    }

    /// Allows or refuses what the plugin's current `ready` asked for and the
    /// person has not decided yet. Allowed accounts and presence go out now.
    fn decide_plugin_needs(&mut self, id: &str, allow: bool) {
        self.decide_plugin_needs_with(id, &|_| allow);
    }

    /// Allows the pending needs `allowed` accepts and refuses the other
    /// pending ones. Allowed accounts and presence go out now.
    fn decide_plugin_needs_with(&mut self, id: &str, allowed: &dyn Fn(&str) -> bool) {
        let pending = match self.plugins.get_mut(id) {
            Some(state) if !state.pending_needs.is_empty() => {
                std::mem::take(&mut state.pending_needs)
            }
            _ => return,
        };
        let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) else {
            return;
        };
        let mut any_allowed = false;
        for need in pending {
            let allow = allowed(need);
            any_allowed |= allow;
            let (add, other) = if allow {
                (&mut entry.approved_needs, &mut entry.denied_needs)
            } else {
                (&mut entry.denied_needs, &mut entry.approved_needs)
            };
            other.retain(|n| n != need);
            if !add.iter().any(|n| n == need) {
                add.push(need.to_owned());
            }
        }
        self.mark_settings_dirty();
        self.sync_push_route(false);
        if !any_allowed {
            return;
        }
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return;
        };
        if Self::plugin_needs_accounts(entry)
            && self.plugins_started.contains_key(id)
            && self
                .plugins
                .get(id)
                .is_some_and(|state| state.running && state.sent_accounts.is_none())
        {
            let forget = entry.forget.clone();
            let accounts = self.plugin_accounts();
            if let Some(state) = self.plugins.get_mut(id) {
                state.sent_accounts = Some(accounts.clone());
            }
            self.send_plugin_accounts(id, accounts);
            if !forget.is_empty() {
                self.send_service(Command::PluginHello {
                    id: id.to_owned(),
                    accounts: None,
                    forget: Some(forget),
                });
            }
        }
        self.sync_plugin_presence();
    }

    /// Runs a plugin file that no longer matches its stored hash: the new
    /// hash becomes the pin, what the plugin was allowed is forgotten (the new
    /// program asks again) and the plugin restarts.
    fn trust_plugin_file(&mut self, id: &str) {
        let Some(sha256) = self
            .plugins
            .get_mut(id)
            .and_then(|state| state.file_changed.take())
        else {
            return;
        };
        let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) else {
            return;
        };
        entry.sha256 = Some(sha256);
        entry.approved_needs.clear();
        entry.denied_needs.clear();
        self.mark_settings_dirty();
        if let Some(state) = self.plugins.get_mut(id) {
            state.pending_needs.clear();
            state.notified_pending.clear();
        }
        self.plugins_started.remove(id);
        self.sync_plugins();
    }

    /// Records whether a plugin secret was stored. An answer for another
    /// instance of the plugin (it was removed and added again meanwhile) is
    /// ignored.
    pub(super) fn on_plugin_secret_saved(
        &mut self,
        plugin: &str,
        instance: &str,
        option: String,
        set: bool,
        ok: bool,
    ) {
        if !self
            .settings
            .plugins
            .iter()
            .any(|e| e.id == plugin && e.instance == instance)
        {
            return;
        }
        if !ok {
            self.toast(
                ToastKind::Error,
                "A plugin's secret could not be saved in the system's credential store.",
            );
            return;
        }
        let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == plugin) else {
            return;
        };
        entry.secrets.retain(|stored| *stored != option);
        if set {
            entry.secrets.push(option);
        }
        self.mark_settings_dirty();
        self.send_plugin_options(plugin);
    }
}

/// Chaat's short words for a pending need, in the row summary.
fn need_short_words(need: &str) -> &'static str {
    match need {
        "accounts" => "your accounts",
        "presence" => "your presence",
        registry::PUSH => "your update notifications",
        _ => "something new",
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

impl App {
    /// The capabilities this plugin provides now, push first: it is the first
    /// enabled, running plugin that offers them. Push does not come from a
    /// plugin while a manual relay address is set.
    pub fn plugin_provides(&self, id: &str) -> Vec<&'static str> {
        let mut provides = Vec::new();
        if self.settings.relay_url.is_none() && self.push_owner().as_deref() == Some(id) {
            provides.push(registry::PUSH);
        }
        if self.call_alerts_owner().as_deref() == Some(id) {
            provides.push(registry::CALL_ALERTS);
        }
        provides
    }

    /// The first other plugin, in list order, that is enabled, running and
    /// offers `capability`: the one that would take over.
    fn plugin_next_provider(&self, id: &str, capability: &str) -> Option<String> {
        self.settings
            .plugins
            .iter()
            .filter(|entry| entry.enabled && entry.id != id)
            .find(|entry| {
                self.plugins
                    .get(&entry.id)
                    .is_some_and(|state| state.running && state.offers.contains(&capability))
            })
            .map(|entry| entry.id.clone())
    }

    /// The question to ask before turning this plugin off, in Chaat's words;
    /// `None` when it provides nothing.
    pub fn turn_off_question(&self, id: &str) -> Option<String> {
        let consequences = self.turn_off_consequences(id)?;
        Some(format!(
            "Turn off {}? {consequences}",
            self.quoted_plugin_name(id)
        ))
    }

    /// A plugin's name in quotation marks, so its words stay apart from
    /// Chaat's sentence. Quotation marks inside the name become apostrophes,
    /// so it cannot close the quote early.
    pub fn quoted_plugin_name(&self, id: &str) -> String {
        let name = self.plugin_name(id).replace(['\u{201C}', '\u{201D}'], "'");
        format!("\u{201C}{name}\u{201D}")
    }

    /// What stops or who takes over when this plugin is turned off, in
    /// Chaat's words; `None` when it provides nothing.
    pub fn turn_off_consequences(&self, id: &str) -> Option<String> {
        let provides = self.plugin_provides(id);
        let mut sentences = Vec::new();
        for capability in provides {
            let other = self.plugin_next_provider(id, capability);
            sentences.push(match (capability, other) {
                (registry::PUSH, Some(other)) => {
                    format!(
                        "{} provides push updates instead.",
                        self.quoted_plugin_name(&other)
                    )
                }
                (registry::PUSH, None) => {
                    "Push updates stop until you turn it back on or set a relay address.".to_owned()
                }
                (_, Some(other)) => {
                    format!(
                        "{} provides call alerts instead.",
                        self.quoted_plugin_name(&other)
                    )
                }
                (_, None) => "Chaat stops showing incoming calls from this plugin.".to_owned(),
            });
        }
        if sentences.is_empty() {
            return None;
        }
        Some(sentences.join(" "))
    }

    /// Records that the plugin was started `ago` before now, as the add flow
    /// waits on it (tests).
    pub fn pretend_plugin_started(&mut self, id: &str, ago: Duration) {
        let Some(path) = self
            .settings
            .plugins
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.path.clone())
        else {
            return;
        };
        self.plugins_started.insert(id.to_owned(), path);
        if let Some(at) = Instant::now().checked_sub(ago) {
            self.plugin_started_at.insert(id.to_owned(), at);
        }
    }

    /// How long the add flow's start of this plugin may still wait for its
    /// `ready`; `None` when no start is being waited on (not started, or
    /// `ready` came). Zero once the limit has passed.
    pub fn plugin_start_remaining(&self, id: &str) -> Option<Duration> {
        let at = self.plugin_started_at.get(id)?;
        if self.plugins.get(id).is_some_and(|state| state.ready_seen) {
            return None;
        }
        Some(ADD_START_LIMIT.saturating_sub(at.elapsed()))
    }

    /// Whether the plugin's current run has sent `ready` (so its needs and
    /// options are known).
    pub fn plugin_ready(&self, id: &str) -> bool {
        self.plugins
            .get(id)
            .is_some_and(|state| state.running && state.ready_seen)
    }

    /// Whether a plugin started for its add flow has waited too long for its
    /// `ready`.
    pub fn plugin_start_timed_out(&self, id: &str) -> bool {
        self.plugin_started_at
            .get(id)
            .is_some_and(|at| at.elapsed() >= ADD_START_LIMIT)
            && !self.plugins.get(id).is_some_and(|state| state.ready_seen)
    }

    /// One line about a plugin for the plugins list. The first matching rule
    /// of the spec's table wins.
    pub fn plugin_summary(&self, id: &str) -> PluginSummary {
        let make = |level, text: &str| PluginSummary {
            level,
            text: text.to_owned(),
        };
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return make(SummaryLevel::Off, "Not in the list");
        };
        let state = self.plugins.get(id);
        if entry.setup_pending {
            let stopped = state.is_some_and(|s| !s.ready_seen && s.message.is_some());
            return if self.plugins_started.contains_key(id)
                && (stopped || self.plugin_start_timed_out(id))
            {
                make(SummaryLevel::Error, "It stopped before saying what it does")
            } else {
                make(SummaryLevel::Attention, "Finish setup")
            };
        }
        if state.is_some_and(|s| s.file_changed.is_some()) {
            return make(SummaryLevel::Attention, "The file changed. Review it");
        }
        if let Some(need) = state.and_then(|s| s.pending_needs.first()) {
            return PluginSummary {
                level: SummaryLevel::Attention,
                text: format!("Asks for {}", need_short_words(need)),
            };
        }
        if !self.plugin_missing_required(id).is_empty() {
            return make(SummaryLevel::Attention, "Needs setup");
        }
        let running = state.is_some_and(|s| s.running);
        if entry.enabled
            && !running
            && let Some(message) = state.and_then(|s| s.message.as_deref())
        {
            return PluginSummary {
                level: SummaryLevel::Error,
                text: format!("Stopped: {message}"),
            };
        }
        if !entry.enabled {
            return make(SummaryLevel::Off, "Off");
        }
        if !running {
            return make(SummaryLevel::Off, "Starting\u{2026}");
        }
        let state = state.expect("a running plugin has state");
        if self.call_alerts_owner().as_deref() == Some(id) {
            if !state.listening.is_empty() {
                return PluginSummary {
                    level: SummaryLevel::Ok,
                    text: format!(
                        "Listening on {}",
                        plural(state.listening.len(), "account", "accounts")
                    ),
                };
            }
            if !state.sign_in.is_empty() {
                return PluginSummary {
                    level: SummaryLevel::Ok,
                    text: format!("Waiting for sign-in on {}", state.sign_in.len()),
                };
            }
        }
        // With a manual relay address push does not come from a plugin.
        let push_by_plugin = self.settings.relay_url.is_none();
        if push_by_plugin
            && self.push_owner().as_deref() == Some(id)
            && let Some(view) = self.push_plugin_view()
        {
            match view.state {
                PushPluginState::Up { host } => {
                    return PluginSummary {
                        level: SummaryLevel::Ok,
                        text: format!("Tunnel at {host}"),
                    };
                }
                PushPluginState::Waiting => {
                    return make(SummaryLevel::Ok, "Waiting for its tunnel");
                }
                PushPluginState::Down { .. } => return make(SummaryLevel::Ok, "Tunnel down"),
                PushPluginState::NeedsApproval => {}
            }
        }
        let loses_calls = state.offers.contains(&registry::CALL_ALERTS)
            && self.call_alerts_owner().as_deref() != Some(id);
        let loses_push = push_by_plugin
            && state.offers.contains(&registry::PUSH)
            && self.push_owner().as_deref() != Some(id);
        if loses_calls || loses_push {
            let what = if loses_calls { "calls" } else { "push" };
            return PluginSummary {
                level: SummaryLevel::Off,
                text: format!("Not used: another plugin provides {what}"),
            };
        }
        make(SummaryLevel::Ok, "Running")
    }

    /// Opens a plugin's page, or its add flow while its setup is pending.
    fn open_plugin(&mut self, id: String) {
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return;
        };
        self.plugins_view = if entry.setup_pending {
            PluginsView::Adding(id)
        } else {
            PluginsView::Plugin(id)
        };
        self.reordering = false;
        self.settings_page = Some(SettingsPage::Plugins);
        self.last_settings_page = SettingsPage::Plugins;
        self.overlay = None;
        self.sync_plugins();
    }

    /// Keeps the plugins view to Settings › Plugins: leaving it, or the
    /// plugin going away, returns to the list and ends the reorder mode. An
    /// unfinished add flow stops its plugin.
    pub(super) fn plugins_view_follows_settings(&mut self) {
        let gone = match &self.plugins_view {
            PluginsView::List => false,
            PluginsView::Plugin(id) | PluginsView::Adding(id) => {
                !self.settings.plugins.iter().any(|e| &e.id == id)
            }
        };
        if self.settings_page != Some(SettingsPage::Plugins) || gone {
            self.reordering = false;
            if self.plugins_view != PluginsView::List {
                self.plugins_view = PluginsView::List;
                self.sync_plugins();
            }
        }
    }

    /// Moves a plugin to index `to` (clamped), saves, and moves the push
    /// route and call alerts to whichever plugin now comes first, as a stop
    /// would.
    fn move_plugin(&mut self, id: &str, to: usize) {
        let list = &mut self.settings.plugins;
        let Some(from) = list.iter().position(|e| e.id == id) else {
            return;
        };
        let to = to.min(list.len() - 1);
        if from == to {
            return;
        }
        let entry = list.remove(from);
        list.insert(to, entry);
        self.mark_settings_dirty();
        // Sign-in notices belong to the plugin that provides call alerts.
        let owner = self.call_alerts_owner();
        for (plugin, state) in &mut self.plugins {
            if state.offers.contains(&registry::CALL_ALERTS)
                && owner.as_deref() != Some(plugin.as_str())
            {
                state.sign_in.clear();
                state.asked.clear();
            }
        }
        self.sync_push_route(false);
    }

    /// Ends the add flow: the ticked needs are allowed, the other pending ones
    /// refused, and the plugin is a finished one. Nothing is sent for a need
    /// that was not ticked. Ignored while required options are empty.
    fn finish_plugin_setup(&mut self, id: &str, allow: &[String]) {
        if !self
            .settings
            .plugins
            .iter()
            .any(|e| e.id == id && e.setup_pending)
            || !self.plugin_missing_required(id).is_empty()
        {
            return;
        }
        self.decide_plugin_needs_with(id, &|need| allow.iter().any(|a| a == need));
        if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) {
            entry.setup_pending = false;
        }
        self.mark_settings_dirty();
        self.plugins_view = PluginsView::Plugin(id.to_owned());
        self.sync_plugins();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Waker};
    use crate::paths::AppDirs;
    use crate::settings::Settings;

    #[test]
    fn a_ready_that_drops_accounts_clears_the_stored_restart_hello() {
        let (backend, _) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-unused");
        let mut app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            false,
        );
        let (service, mut commands) = Backend::recording();
        app.service = service;
        app.id = Some("u-me.t1".into());
        app.settings.plugins.push(PluginEntry {
            id: "p".into(),
            approved_needs: vec!["accounts".into()],
            ..PluginEntry::default()
        });
        let ready = |app: &mut App, needs: Vec<&'static str>| {
            app.on_plugin_ready(
                "p",
                PluginInfo::default(),
                Vec::new(),
                needs,
                Vec::new(),
                Vec::new(),
            );
        };
        ready(&mut app, vec!["accounts"]);
        while commands.try_recv().is_ok() {}
        ready(&mut app, Vec::new());
        let sent: Vec<Command> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(sent.iter().any(|c| matches!(
            c,
            Command::PluginHello { id, accounts: Some(a), .. } if id == "p" && a.is_empty()
        )));
    }

    fn option_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
        let (backend, _) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-unused");
        let mut app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            false,
        );
        let (service, commands) = Backend::recording();
        app.service = service;
        app.settings.plugins.push(PluginEntry {
            id: "p".into(),
            instance: "inst".into(),
            path: PathBuf::from("p.exe"),
            enabled: true,
            sha256: Some("old".into()),
            ..PluginEntry::default()
        });
        (app, commands)
    }

    fn describe_options(app: &mut App) {
        let spec = |id: &str, kind| OptionSpec {
            id: id.into(),
            kind,
            label: id.into(),
            default: None,
            choices: Vec::new(),
            ..OptionSpec::default()
        };
        app.on_plugin_ready(
            "p",
            PluginInfo::default(),
            Vec::new(),
            Vec::new(),
            vec![
                spec("key", OptionKind::Secret),
                spec("name", OptionKind::Text),
            ],
            Vec::new(),
        );
    }

    fn option_commands(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> usize {
        std::iter::from_fn(|| commands.try_recv().ok())
            .filter(|c| matches!(c, Command::PluginOptions { .. }))
            .count()
    }

    fn change_and_answer(app: &mut App) {
        app.apply_plugin_action(Action::SetPluginOption {
            plugin: "p".into(),
            option: "name".into(),
            value: serde_json::json!("x"),
        });
        app.on_plugin_secret_saved("p", "inst", "key".into(), true, true);
    }

    #[test]
    fn no_options_are_sent_before_the_new_runs_ready() {
        let (mut app, mut commands) = option_app();
        app.sync_plugins();
        describe_options(&mut app);
        while commands.try_recv().is_ok() {}
        change_and_answer(&mut app);
        assert!(option_commands(&mut commands) > 0, "after ready they go");
        // A restart: the old run's options are not sent to the new program.
        app.settings.plugins[0].path = PathBuf::from("q.exe");
        app.sync_plugins();
        change_and_answer(&mut app);
        assert_eq!(option_commands(&mut commands), 0);
        describe_options(&mut app);
        assert!(option_commands(&mut commands) > 0, "sent again on ready");
    }

    #[test]
    fn trusting_a_changed_file_sends_options_only_after_ready() {
        let (mut app, mut commands) = option_app();
        app.sync_plugins();
        describe_options(&mut app);
        app.plugins.get_mut("p").unwrap().file_changed = Some("new".into());
        while commands.try_recv().is_ok() {}
        app.trust_plugin_file("p");
        change_and_answer(&mut app);
        assert_eq!(option_commands(&mut commands), 0);
        describe_options(&mut app);
        assert!(option_commands(&mut commands) > 0);
    }

    fn generation_of(command: Option<Command>) -> u64 {
        match command {
            Some(Command::StartPlugin { generation, .. }) => generation,
            other => panic!("expected a start, got {other:?}"),
        }
    }

    #[test]
    fn events_from_an_old_generation_are_dropped() {
        let (backend, _) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-unused");
        let mut app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            false,
        );
        let (service, mut commands) = Backend::recording();
        app.service = service;
        app.settings.plugins.push(PluginEntry {
            id: "p".into(),
            path: PathBuf::from("p.exe"),
            enabled: true,
            ..PluginEntry::default()
        });
        let running = |app: &mut App, generation: u64, running: bool| {
            app.on_plugin_event(
                "p".into(),
                generation,
                PluginEvent::Status {
                    running,
                    message: None,
                },
            );
            app.plugins["p"].running
        };
        app.sync_plugins();
        let first = generation_of(commands.try_recv().ok());
        assert!(running(&mut app, first, true), "the current run counts");
        // A new path restarts the plugin: the first run's last words are old.
        app.settings.plugins[0].path = PathBuf::from("q.exe");
        app.sync_plugins();
        let second = generation_of(commands.try_recv().ok());
        assert_ne!(first, second);
        assert!(!app.plugins["p"].running, "a restart forgets the old run");
        assert!(!running(&mut app, first, true), "the old run is ignored");
        assert!(running(&mut app, second, true), "the new run counts");
        // Turned off: nothing the stopped host still says counts, not even
        // from the latest generation.
        app.settings.plugins[0].enabled = false;
        app.sync_plugins();
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::StopPlugin { id }) if id == "p"
        ));
        assert!(!running(&mut app, second, true));
        // Turned on again: a newer generation, and only that one counts.
        app.settings.plugins[0].enabled = true;
        app.sync_plugins();
        let third = generation_of(commands.try_recv().ok());
        assert!(third > second);
        assert!(!running(&mut app, second, true));
        assert!(running(&mut app, third, true));
        // Removed: events from the plugin are dropped.
        app.remove_plugin("p");
        app.plugins.entry("p".into()).or_default().running = false;
        assert!(!running(&mut app, third, true));
    }

    // Options: statuses, choices, checks, actions, reset, setup.

    use crate::plugin::protocol::OptionChoice;
    use serde_json::json;

    fn spec(id: &str, kind: OptionKind) -> OptionSpec {
        OptionSpec {
            id: id.into(),
            kind,
            label: id.into(),
            ..OptionSpec::default()
        }
    }

    fn choice_spec(id: &str, values: &[&str]) -> OptionSpec {
        OptionSpec {
            choices: values
                .iter()
                .map(|v| OptionChoice {
                    value: (*v).into(),
                    label: (*v).into(),
                })
                .collect(),
            ..spec(id, OptionKind::Choice)
        }
    }

    fn number_spec(id: &str, min: f64, max: f64, default: f64) -> OptionSpec {
        OptionSpec {
            min: Some(min),
            max: Some(max),
            default: Some(json!(default)),
            ..spec(id, OptionKind::Number)
        }
    }

    fn ready_with(app: &mut App, specs: Vec<OptionSpec>) {
        app.on_plugin_ready(
            "p",
            PluginInfo::default(),
            Vec::new(),
            Vec::new(),
            specs,
            Vec::new(),
        );
    }

    fn drain(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| commands.try_recv().ok()).collect()
    }

    fn plugin_event(app: &mut App, event: PluginEvent) {
        let generation = app.plugin_generations.get("p").copied().unwrap_or(0);
        app.on_plugin_event("p".into(), generation, event);
    }

    fn status(app: &mut App, id: &str, state: StatusState, message: &str) {
        plugin_event(
            app,
            PluginEvent::OptionStatus {
                id: id.into(),
                state,
                message: Some(message.into()),
            },
        );
    }

    fn sent_values(
        commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) -> Option<BTreeMap<String, serde_json::Value>> {
        drain(commands).into_iter().rev().find_map(|c| match c {
            Command::PluginOptions { values, .. } => Some(values),
            _ => None,
        })
    }

    fn action_sent(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> usize {
        drain(commands)
            .into_iter()
            .filter(|c| {
                matches!(
                    c,
                    Command::PluginSend {
                        message: HostMessage::Action { .. },
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn the_latest_status_wins_and_restart_clears_them() {
        let (mut app, _commands) = option_app();
        app.sync_plugins();
        ready_with(&mut app, vec![spec("scan", OptionKind::Action)]);
        for n in 1..=7 {
            status(&mut app, "scan", StatusState::Ok, &format!("m{n}"));
        }
        assert_eq!(
            app.plugins["p"].statuses["scan"].1.as_deref(),
            Some("m7"),
            "a burst never leaves an older status on screen"
        );
        // Only options the current `ready` describes have a status.
        status(&mut app, "unknown", StatusState::Ok, "x");
        assert!(!app.plugins["p"].statuses.contains_key("unknown"));
        // A restart forgets them.
        app.settings.plugins[0].path = PathBuf::from("q.exe");
        app.sync_plugins();
        assert!(app.plugins["p"].statuses.is_empty());
        status(&mut app, "scan", StatusState::Ok, "stale");
        assert!(
            app.plugins["p"].statuses.is_empty(),
            "nothing counts before the new run's ready"
        );
    }

    #[test]
    fn options_update_replaces_choices_and_keeps_old_value() {
        let (mut app, _commands) = option_app();
        app.sync_plugins();
        app.settings.plugins[0]
            .options
            .insert("bulb".into(), json!("a"));
        ready_with(
            &mut app,
            vec![choice_spec("bulb", &["a"]), spec("name", OptionKind::Text)],
        );
        let update = |id: &str, values: &[&str]| PluginEvent::OptionsUpdate {
            id: id.into(),
            choices: values
                .iter()
                .map(|v| OptionChoice {
                    value: (*v).into(),
                    label: (*v).into(),
                })
                .collect(),
        };
        plugin_event(&mut app, update("bulb", &["b", "c"]));
        let values: Vec<&str> = app.plugins["p"].options[0]
            .choices
            .iter()
            .map(|c| c.value.as_str())
            .collect();
        assert_eq!(values, ["b", "c"]);
        assert_eq!(
            app.settings.plugins[0].options["bulb"],
            json!("a"),
            "the stored value stays"
        );
        // Not a choice option, or not described: ignored.
        plugin_event(&mut app, update("name", &["x"]));
        plugin_event(&mut app, update("nope", &["x"]));
        assert!(app.plugins["p"].options[1].choices.is_empty());
        assert_eq!(app.plugins["p"].options.len(), 2);
    }

    #[test]
    fn invalid_values_are_not_stored_or_sent() {
        let (mut app, mut commands) = option_app();
        app.sync_plugins();
        ready_with(
            &mut app,
            vec![
                number_spec("bright", 10.0, 100.0, 50.0),
                spec("name", OptionKind::Text),
                spec("key", OptionKind::Secret),
            ],
        );
        drain(&mut commands);
        let set = |app: &mut App, option: &str, value| {
            app.apply_plugin_action(Action::SetPluginOption {
                plugin: "p".into(),
                option: option.into(),
                value,
            });
        };
        let error = |app: &App, option: &str| {
            app.plugin_option_errors
                .get(&("p".to_owned(), option.to_owned()))
                .cloned()
        };
        set(&mut app, "bright", json!(5));
        assert_eq!(
            error(&app, "bright").as_deref(),
            Some("Enter a number from 10 to 100.")
        );
        assert!(!app.settings.plugins[0].options.contains_key("bright"));
        assert!(sent_values(&mut commands).is_none(), "nothing is sent");
        // A secret never becomes a stored value, and neither does an unknown
        // option.
        set(&mut app, "key", json!("k3y"));
        set(&mut app, "nope", json!("x"));
        assert!(app.settings.plugins[0].options.is_empty());
        assert!(sent_values(&mut commands).is_none());
        // A valid value clears the error; only the normalized value is kept.
        set(&mut app, "bright", json!(60));
        assert_eq!(error(&app, "bright"), None);
        assert_eq!(app.settings.plugins[0].options["bright"], json!(60));
        assert_eq!(sent_values(&mut commands).unwrap()["bright"], json!(60));
        set(&mut app, "name", json!("  hi "));
        assert_eq!(app.settings.plugins[0].options["name"], json!("hi"));
    }

    #[test]
    fn stored_values_that_no_longer_fit_are_dropped_on_ready() {
        let (mut app, mut commands) = option_app();
        app.sync_plugins();
        {
            let stored = &mut app.settings.plugins[0].options;
            stored.insert("bright".into(), json!(500));
            stored.insert("bulb".into(), json!(7));
            stored.insert("name".into(), json!("ok"));
            stored.insert("old".into(), json!("kept"));
        }
        ready_with(
            &mut app,
            vec![
                number_spec("bright", 10.0, 100.0, 50.0),
                choice_spec("bulb", &["a"]),
                spec("name", OptionKind::Text),
            ],
        );
        let stored = &app.settings.plugins[0].options;
        assert!(!stored.contains_key("bright"));
        assert!(!stored.contains_key("bulb"), "not a choice value");
        assert_eq!(stored["name"], json!("ok"));
        assert!(stored.contains_key("old"), "undescribed values are kept");
        let values = sent_values(&mut commands).expect("options after ready");
        assert_eq!(values["bright"], json!(50.0), "the default stands in");
        assert_eq!(values["name"], json!("ok"));
        assert!(!values.contains_key("bulb"));
    }

    #[test]
    fn actions_go_only_to_a_ready_plugin() {
        let (mut app, mut commands) = option_app();
        let run = |app: &mut App, action: &str| {
            app.apply_plugin_action(Action::RunPluginAction {
                plugin: "p".into(),
                action: action.into(),
            });
        };
        app.sync_plugins();
        drain(&mut commands);
        run(&mut app, "scan");
        assert_eq!(action_sent(&mut commands), 0, "not before a ready");
        ready_with(
            &mut app,
            vec![
                spec("scan", OptionKind::Action),
                spec("name", OptionKind::Text),
            ],
        );
        drain(&mut commands);
        run(&mut app, "name");
        run(&mut app, "nope");
        assert_eq!(action_sent(&mut commands), 0, "only a described button");
        run(&mut app, "scan");
        let sent = drain(&mut commands);
        assert!(sent.iter().any(|c| matches!(
            c,
            Command::PluginSend {
                id,
                message: HostMessage::Action { id: action },
            } if id == "p" && action == "scan"
        )));
        assert!(app.plugins["p"].action_started.contains_key("scan"));
        // Not running: no click goes out.
        app.plugins.get_mut("p").unwrap().running = false;
        run(&mut app, "scan");
        assert_eq!(action_sent(&mut commands), 0);
        // A restart is not ready until its own ready.
        app.plugins.get_mut("p").unwrap().running = true;
        app.settings.plugins[0].path = PathBuf::from("q.exe");
        app.sync_plugins();
        app.plugins.get_mut("p").unwrap().running = true;
        drain(&mut commands);
        run(&mut app, "scan");
        assert_eq!(action_sent(&mut commands), 0);
    }

    #[test]
    fn an_action_is_working_until_its_status_or_30s() {
        let (mut app, _commands) = option_app();
        app.sync_plugins();
        ready_with(&mut app, vec![spec("scan", OptionKind::Action)]);
        let now = Instant::now();
        assert!(!app.plugins["p"].action_working("scan", now));
        let run = |app: &mut App| {
            app.apply_plugin_action(Action::RunPluginAction {
                plugin: "p".into(),
                action: "scan".into(),
            });
        };
        run(&mut app);
        assert!(app.plugins["p"].action_working("scan", Instant::now()));
        assert!(
            !app.plugins["p"].action_working("scan", Instant::now() + Duration::from_secs(31)),
            "30 seconds without an answer ends the wait"
        );
        status(&mut app, "scan", StatusState::Working, "Looking");
        assert!(app.plugins["p"].action_working("scan", Instant::now()));
        status(&mut app, "scan", StatusState::Ok, "Found 2 bulbs");
        assert!(!app.plugins["p"].action_working("scan", Instant::now()));
        assert!(app.plugins["p"].action_started.is_empty());
        // The same for an error, and a click clears the old answer.
        run(&mut app);
        assert!(!app.plugins["p"].statuses.contains_key("scan"));
        status(&mut app, "scan", StatusState::Error, "No bulbs");
        assert!(!app.plugins["p"].action_working("scan", Instant::now()));
    }

    #[test]
    fn reset_section_restores_defaults_and_keeps_secrets() {
        let (mut app, mut commands) = option_app();
        app.sync_plugins();
        let in_section = |section: &str, spec: OptionSpec| OptionSpec {
            section: Some(section.into()),
            ..spec
        };
        ready_with(
            &mut app,
            vec![
                in_section("Colors", number_spec("bright", 10.0, 100.0, 50.0)),
                in_section("Colors", spec("note", OptionKind::Text)),
                in_section("Colors", spec("key", OptionKind::Secret)),
                in_section("Bulb", number_spec("other", 1.0, 100.0, 10.0)),
            ],
        );
        {
            let entry = &mut app.settings.plugins[0];
            entry.options.insert("bright".into(), json!(70));
            entry.options.insert("note".into(), json!("x"));
            entry.options.insert("other".into(), json!(20));
            entry.secrets = vec!["key".into()];
        }
        app.plugin_option_errors
            .insert(("p".into(), "bright".into()), "Enter a number.".into());
        drain(&mut commands);
        app.apply_plugin_action(Action::ResetPluginSection {
            plugin: "p".into(),
            section: Some("Colors".into()),
        });
        let stored = &app.settings.plugins[0].options;
        assert_eq!(stored["bright"], json!(50.0));
        assert!(!stored.contains_key("note"), "no default: cleared");
        assert_eq!(stored["other"], json!(20), "other sections stay");
        assert_eq!(app.settings.plugins[0].secrets, ["key"]);
        assert!(app.plugin_option_errors.is_empty());
        let sent = drain(&mut commands);
        assert!(sent.iter().any(|c| matches!(
            c,
            Command::PluginOptions { values, secrets, .. }
                if values["bright"] == json!(50.0) && secrets == &["key".to_owned()]
        )));
        assert!(
            !sent
                .iter()
                .any(|c| matches!(c, Command::SavePluginSecret { .. })),
            "secrets are untouched"
        );
    }

    #[test]
    fn a_new_plugin_opens_its_add_flow_and_stays_pending() {
        let (mut app, mut commands) = option_app();
        let required = |id: &str, kind| OptionSpec {
            required: true,
            ..spec(id, kind)
        };
        app.sync_plugins();
        ready_with(&mut app, vec![required("key", OptionKind::Secret)]);
        assert_eq!(
            app.plugins_view,
            PluginsView::List,
            "an old plugin: no flow"
        );
        assert_eq!(app.plugin_missing_required("p"), ["key"]);
        app.settings.plugins[0].secrets = vec!["key".into()];
        assert!(app.plugin_missing_required("p").is_empty());
        app.settings.plugins[0].secrets.clear();
        drain(&mut commands);
        app.add_plugin(PathBuf::from("added-plugin.exe"));
        let id = app.settings.plugins.last().unwrap().id.clone();
        assert!(app.settings.plugins.last().unwrap().setup_pending);
        assert_eq!(app.plugins_view, PluginsView::Adding(id.clone()));
        assert!(
            drain(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::StartPlugin { id: started, .. } if *started == id)),
            "it starts so that it can send ready"
        );
        app.remove_plugin(&id);
        assert_eq!(app.plugins_view, PluginsView::List);
    }

    #[test]
    fn setup_pending_is_not_started_at_launch() {
        let (mut app, mut commands) = option_app();
        app.settings.plugins.push(PluginEntry {
            id: "new".into(),
            path: PathBuf::from("new.exe"),
            enabled: true,
            setup_pending: true,
            ..PluginEntry::default()
        });
        app.sync_plugins();
        let started: Vec<String> = drain(&mut commands)
            .into_iter()
            .filter_map(|c| match c {
                Command::StartPlugin { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(started, ["p"], "only the finished plugin runs");
        assert_eq!(
            app.plugin_summary("new"),
            PluginSummary {
                level: SummaryLevel::Attention,
                text: "Finish setup".into()
            }
        );
        // Opening its setup page starts it; leaving Settings stops it again.
        app.apply_plugin_action(Action::OpenPlugin("new".into()));
        assert_eq!(app.plugins_view, PluginsView::Adding("new".into()));
        assert_eq!(app.settings_page, Some(SettingsPage::Plugins));
        assert!(
            drain(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::StartPlugin { id, .. } if id == "new"))
        );
        app.settings_page = None;
        app.plugins_view_follows_settings();
        assert_eq!(app.plugins_view, PluginsView::List);
        assert!(
            drain(&mut commands)
                .iter()
                .any(|c| matches!(c, Command::StopPlugin { id } if id == "new"))
        );
        assert!(app.settings.plugins[1].setup_pending, "still pending");
    }

    #[test]
    fn old_settings_load_setup_pending_false() {
        let entry: PluginEntry =
            serde_json::from_str(r#"{"id":"p","path":"p.exe","enabled":true}"#).unwrap();
        assert!(!entry.setup_pending);
    }

    fn pending_app() -> App {
        let (mut app, _commands) = option_app();
        app.settings.plugins[0].setup_pending = true;
        app.settings.plugins[0].needs = vec!["accounts".into(), "presence".into()];
        app.sync_plugins_for_test();
        app
    }

    impl App {
        /// Starts "p" as the add flow does, then answers with a `ready` that
        /// asks for accounts and presence.
        fn sync_plugins_for_test(&mut self) {
            self.plugins_view = PluginsView::Adding("p".into());
            self.sync_plugins();
            self.on_plugin_ready(
                "p",
                PluginInfo::default(),
                Vec::new(),
                vec!["accounts", "presence"],
                Vec::new(),
                Vec::new(),
            );
        }
    }

    #[test]
    fn finish_setup_approves_ticked_refuses_unticked() {
        let mut app = pending_app();
        assert_eq!(app.plugins["p"].pending_needs, ["accounts", "presence"]);
        app.apply_plugin_action(Action::FinishPluginSetup {
            id: "p".into(),
            allow: vec!["presence".into()],
        });
        let entry = &app.settings.plugins[0];
        assert!(!entry.setup_pending);
        assert_eq!(entry.approved_needs, ["presence"]);
        assert_eq!(entry.denied_needs, ["accounts"]);
        assert!(app.plugins["p"].pending_needs.is_empty());
        assert_eq!(app.plugins_view, PluginsView::Plugin("p".into()));
        assert!(!App::plugin_need_effective(entry, "accounts"));
        assert!(App::plugin_need_effective(entry, "presence"));
    }

    #[test]
    fn finish_setup_with_nothing_ticked_sends_no_accounts() {
        let mut app = pending_app();
        let (service, mut commands) = Backend::recording();
        app.service = service;
        app.apply_plugin_action(Action::FinishPluginSetup {
            id: "p".into(),
            allow: Vec::new(),
        });
        assert_eq!(app.settings.plugins[0].denied_needs.len(), 2);
        assert!(app.settings.plugins[0].approved_needs.is_empty());
        assert!(!drain(&mut commands).iter().any(|c| matches!(
            c,
            Command::PluginSend {
                message: HostMessage::Accounts { .. } | HostMessage::Presence { .. },
                ..
            }
        )));
    }

    #[test]
    fn finish_setup_waits_for_required_options() {
        let mut app = pending_app();
        app.on_plugin_ready(
            "p",
            PluginInfo::default(),
            Vec::new(),
            Vec::new(),
            vec![OptionSpec {
                required: true,
                ..spec("name", OptionKind::Text)
            }],
            Vec::new(),
        );
        app.apply_plugin_action(Action::FinishPluginSetup {
            id: "p".into(),
            allow: Vec::new(),
        });
        assert!(
            app.settings.plugins[0].setup_pending,
            "a required option is empty"
        );
        assert_eq!(app.plugins_view, PluginsView::Adding("p".into()));
    }

    #[test]
    fn cancel_setup_removes() {
        let mut app = pending_app();
        app.apply_plugin_action(Action::CancelPluginSetup("p".into()));
        assert!(app.settings.plugins.is_empty());
        assert!(!app.plugins.contains_key("p"));
        assert_eq!(app.plugins_view, PluginsView::List);
        // A finished plugin is not removed by a stray cancel.
        let (mut app, _commands) = option_app();
        app.apply_plugin_action(Action::CancelPluginSetup("p".into()));
        assert_eq!(app.settings.plugins.len(), 1);
    }

    #[test]
    fn leaving_settings_resets_the_view_and_reordering() {
        let (mut app, _commands) = option_app();
        app.settings.plugins.push(PluginEntry {
            id: "q".into(),
            ..PluginEntry::default()
        });
        app.apply_plugin_action(Action::OpenPlugin("p".into()));
        assert_eq!(app.plugins_view, PluginsView::Plugin("p".into()));
        app.apply_plugin_action(Action::BackToPlugins);
        app.apply_plugin_action(Action::StartReorder);
        assert!(app.reordering);
        app.apply_plugin_action(Action::EndReorder);
        assert!(!app.reordering);
        app.apply_plugin_action(Action::StartReorder);
        app.settings_page = None;
        app.plugins_view_follows_settings();
        assert!(!app.reordering);
        // Reordering needs two plugins.
        app.settings.plugins.pop();
        app.apply_plugin_action(Action::StartReorder);
        assert!(!app.reordering);
    }

    /// Two plugins that offer push and call alerts, both running and
    /// allowed, in an app with one account whose backend is returned.
    fn two_providers() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let (mut app, mut service) = option_app();
        app.settings.plugins.clear();
        let (account_backend, mut account) = Backend::recording();
        app.accounts.push(crate::app::Account::new(account_backend));
        for id in ["a", "b"] {
            app.settings.plugins.push(PluginEntry {
                id: id.into(),
                path: PathBuf::from(format!("{id}.exe")),
                enabled: true,
                needs: vec!["push".into()],
                approved_needs: vec!["push".into()],
                ..PluginEntry::default()
            });
            app.plugins.insert(
                id.into(),
                PluginState {
                    running: true,
                    offers: vec![registry::CALL_ALERTS, registry::PUSH],
                    needs: vec!["push"],
                    ready_seen: true,
                    ..PluginState::default()
                },
            );
            app.plugins_started
                .insert(id.into(), PathBuf::from(format!("{id}.exe")));
            let url = format!("https://{id}.trycloudflare.com");
            app.on_plugin_event(
                id.into(),
                0,
                PluginEvent::Push(crate::plugin::event::PushEvent::Ready { url }),
            );
        }
        drain(&mut service);
        drain(&mut account);
        (app, service, account)
    }

    #[test]
    fn move_plugin_switches_owners() {
        let (mut app, mut service, mut account) = two_providers();
        assert_eq!(app.push_owner().as_deref(), Some("a"));
        assert_eq!(app.call_alerts_owner().as_deref(), Some("a"));
        assert_eq!(
            app.plugin_provides("a"),
            [registry::PUSH, registry::CALL_ALERTS]
        );
        assert!(app.plugin_provides("b").is_empty());
        app.plugins
            .get_mut("a")
            .unwrap()
            .sign_in
            .push(PluginSignIn {
                plugin: "a".into(),
                username: None,
                account: None,
                verification_uri: "https://x".into(),
                user_code: "C".into(),
            });
        // Moving to the same place changes nothing.
        app.apply_plugin_action(Action::MovePlugin {
            id: "a".into(),
            to: 0,
        });
        assert!(drain(&mut service).is_empty() && drain(&mut account).is_empty());
        assert_eq!(app.plugins["a"].sign_in.len(), 1);
        app.apply_plugin_action(Action::MovePlugin {
            id: "b".into(),
            to: 0,
        });
        let ids: Vec<&str> = app.settings.plugins.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
        assert_eq!(app.push_owner().as_deref(), Some("b"));
        assert_eq!(app.call_alerts_owner().as_deref(), Some("b"));
        assert_eq!(
            app.plugin_provides("b"),
            [registry::PUSH, registry::CALL_ALERTS]
        );
        assert!(app.plugin_provides("a").is_empty());
        assert!(
            app.plugins["a"].sign_in.is_empty(),
            "the old owner's cards go"
        );
        let sent = drain(&mut service);
        assert!(sent.iter().any(|c| matches!(
            c,
            Command::PluginSend { id, message: HostMessage::PushChannels { channels } }
                if id == "a" && channels.is_empty()
        )));
        assert!(sent.iter().any(|c| matches!(
            c,
            Command::PluginSend { id, message: HostMessage::PushChannels { channels } }
                if id == "b" && channels.len() == app.accounts.len()
        )));
        assert!(drain(&mut account).iter().any(|c| matches!(
            c,
            Command::PushPlugin(Some(route)) if format!("{route:?}").contains("b.trycloudflare.com")
        )));
        // An index past the end moves it last.
        app.apply_plugin_action(Action::MovePlugin {
            id: "b".into(),
            to: 99,
        });
        assert_eq!(app.settings.plugins[1].id, "b");
        assert_eq!(app.push_owner().as_deref(), Some("a"));
    }

    #[test]
    fn plugin_provides_leaves_push_out_while_a_relay_is_set() {
        let (mut app, ..) = two_providers();
        app.settings.relay_url = Some("https://relay.example".into());
        assert_eq!(app.plugin_provides("a"), [registry::CALL_ALERTS]);
    }

    #[test]
    fn summary_ignores_push_while_a_relay_is_set() {
        let (mut app, ..) = two_providers();
        app.plugins.get_mut("a").unwrap().offers = vec![registry::PUSH];
        app.plugins.get_mut("b").unwrap().offers = vec![registry::PUSH];
        assert_eq!(
            app.plugin_summary("a").text,
            "Tunnel at a.trycloudflare.com"
        );
        assert_eq!(
            app.plugin_summary("b").text,
            "Not used: another plugin provides push"
        );
        app.settings.relay_url = Some("https://relay.example".into());
        assert_eq!(app.plugin_summary("a").text, "Running");
        assert_eq!(app.plugin_summary("b").text, "Running");
    }

    #[test]
    fn move_plugin_ignores_unknown_ids_and_same_index() {
        let (mut app, mut service, _account) = two_providers();
        app.apply_plugin_action(Action::MovePlugin {
            id: "nope".into(),
            to: 0,
        });
        app.apply_plugin_action(Action::MovePlugin {
            id: "b".into(),
            to: 1,
        });
        app.apply_plugin_action(Action::MovePlugin {
            id: "b".into(),
            to: 99,
        });
        let ids: Vec<&str> = app.settings.plugins.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert!(drain(&mut service).is_empty());
    }

    #[test]
    fn add_start_times_out_after_the_limit_without_ready() {
        let (mut app, ..) = option_app();
        assert!(!app.plugin_start_timed_out("p"), "never started");
        app.plugin_started_at.insert("p".into(), Instant::now());
        assert!(!app.plugin_start_timed_out("p"));
        if let Some(past) = Instant::now().checked_sub(ADD_START_LIMIT + Duration::from_secs(1)) {
            app.plugin_started_at.insert("p".into(), past);
            app.plugins.entry("p".into()).or_default().ready_seen = false;
            assert!(app.plugin_start_timed_out("p"));
            app.plugins.get_mut("p").unwrap().ready_seen = true;
            assert!(!app.plugin_start_timed_out("p"), "ready ends the wait");
        }
    }

    #[test]
    fn turn_off_question_texts() {
        let (mut app, ..) = two_providers();
        app.plugins.get_mut("a").unwrap().info.name = Some("Alpha".into());
        app.plugins.get_mut("b").unwrap().info.name = Some("Beta".into());
        // Provides nothing: no question.
        assert_eq!(app.turn_off_question("b"), None);
        assert_eq!(app.turn_off_question("nope"), None);
        // Both, another takes over.
        assert_eq!(
            app.turn_off_question("a").as_deref(),
            Some(
                "Turn off “Alpha”? “Beta” provides push updates instead. “Beta” provides call alerts instead."
            )
        );
        // Push only, nothing else offers it; call alerts none either.
        app.plugins.get_mut("b").unwrap().offers.clear();
        assert_eq!(
            app.turn_off_question("a").as_deref(),
            Some(
                "Turn off “Alpha”? Push updates stop until you turn it back on or set a relay address. Chaat stops showing incoming calls from this plugin."
            )
        );
        // Push only.
        app.plugins.get_mut("a").unwrap().offers = vec![registry::PUSH];
        assert_eq!(
            app.turn_off_question("a").as_deref(),
            Some(
                "Turn off “Alpha”? Push updates stop until you turn it back on or set a relay address."
            )
        );
        // Calls only.
        app.plugins.get_mut("a").unwrap().offers = vec![registry::CALL_ALERTS];
        assert_eq!(
            app.turn_off_question("a").as_deref(),
            Some("Turn off “Alpha”? Chaat stops showing incoming calls from this plugin.")
        );
        // Calls only, another takes over.
        app.plugins.get_mut("b").unwrap().offers = vec![registry::CALL_ALERTS];
        assert_eq!(
            app.turn_off_question("a").as_deref(),
            Some("Turn off “Alpha”? “Beta” provides call alerts instead.")
        );
        // A relay address set: push is left out, and with nothing else
        // provided no question is asked.
        app.plugins.get_mut("a").unwrap().offers = vec![registry::PUSH];
        app.settings.relay_url = Some("https://relay.example".into());
        assert_eq!(app.turn_off_question("a"), None);
    }

    #[test]
    fn turn_off_question_quotes_names_a_plugin_chose() {
        let (mut app, ..) = two_providers();
        app.plugins.get_mut("a").unwrap().info.name = Some("Al\u{201C}pha\u{201D}".into());
        app.plugins.get_mut("b").unwrap().info.name = Some("Be, ta".into());
        assert_eq!(app.quoted_plugin_name("a"), "\u{201C}Al'pha'\u{201D}");
        assert_eq!(
            app.turn_off_question("a").as_deref(),
            Some(
                "Turn off \u{201C}Al'pha'\u{201D}? \u{201C}Be, ta\u{201D} provides push updates instead. \u{201C}Be, ta\u{201D} provides call alerts instead."
            )
        );
        assert_eq!(
            app.turn_off_consequences("a").as_deref(),
            Some(
                "\u{201C}Be, ta\u{201D} provides push updates instead. \u{201C}Be, ta\u{201D} provides call alerts instead."
            )
        );
    }

    #[test]
    fn retry_stops_the_old_run_and_ignores_finished_plugins() {
        let (mut app, mut commands) = option_app();
        app.settings.plugins[0].setup_pending = true;
        app.plugins_view = PluginsView::Adding("p".into());
        app.sync_plugins();
        let first = app.plugin_generations["p"];
        let before = app.plugin_started_at["p"];
        while commands.try_recv().is_ok() {}
        app.apply_plugin_action(Action::RetryPluginStart("p".into()));
        let sent: Vec<Command> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        let stop = sent
            .iter()
            .position(|c| matches!(c, Command::StopPlugin { id } if id == "p"));
        let start = sent
            .iter()
            .position(|c| matches!(c, Command::StartPlugin { id, .. } if id == "p"));
        assert!(
            stop.is_some() && start.is_some() && stop < start,
            "stop, then start"
        );
        let second = app.plugin_generations["p"];
        assert!(second > first);
        assert!(
            !app.plugin_event_counts("p", first),
            "a late event from the old run is dropped"
        );
        assert!(app.plugin_event_counts("p", second));
        assert!(app.plugin_started_at["p"] >= before, "the wait restarts");
        // Not in an add flow, or not in the list: nothing happens.
        app.settings.plugins[0].setup_pending = false;
        app.apply_plugin_action(Action::RetryPluginStart("p".into()));
        app.apply_plugin_action(Action::RetryPluginStart("nope".into()));
        assert!(commands.try_recv().is_err());
        assert_eq!(app.plugin_generations["p"], second);
    }

    #[test]
    fn plugin_start_remaining_counts_down_and_ends_with_ready() {
        let (mut app, ..) = option_app();
        assert_eq!(app.plugin_start_remaining("p"), None, "never started");
        app.plugin_started_at.insert("p".into(), Instant::now());
        let left = app.plugin_start_remaining("p").unwrap();
        assert!(left > Duration::ZERO && left <= ADD_START_LIMIT);
        if let Some(past) = Instant::now().checked_sub(ADD_START_LIMIT + Duration::from_secs(1)) {
            app.plugin_started_at.insert("p".into(), past);
            assert_eq!(app.plugin_start_remaining("p"), Some(Duration::ZERO));
            app.plugins.entry("p".into()).or_default().ready_seen = true;
            assert_eq!(app.plugin_start_remaining("p"), None);
        }
    }

    fn summary_of(app: &App, id: &str) -> (SummaryLevel, String) {
        let summary = app.plugin_summary(id);
        (summary.level, summary.text)
    }

    #[test]
    fn summary_order_follows_the_table() {
        use SummaryLevel::*;
        let (mut app, ..) = two_providers();
        // 10: running, offers nothing exclusive.
        app.plugins.get_mut("b").unwrap().offers.clear();
        assert_eq!(summary_of(&app, "b"), (Ok, "Running".into()));
        // 9: another plugin owns what it offers.
        app.plugins.get_mut("b").unwrap().offers = vec![registry::CALL_ALERTS];
        assert_eq!(
            summary_of(&app, "b"),
            (Off, "Not used: another plugin provides calls".into())
        );
        app.plugins.get_mut("b").unwrap().offers = vec![registry::PUSH];
        assert_eq!(
            summary_of(&app, "b"),
            (Off, "Not used: another plugin provides push".into())
        );
        // 8: push owner, tunnel up, then down, then waiting.
        app.plugins.get_mut("a").unwrap().offers = vec![registry::PUSH];
        assert_eq!(
            summary_of(&app, "a"),
            (Ok, "Tunnel at a.trycloudflare.com".into())
        );
        app.on_plugin_event(
            "a".into(),
            0,
            PluginEvent::Push(crate::plugin::event::PushEvent::Down { reason: None }),
        );
        assert_eq!(summary_of(&app, "a"), (Ok, "Tunnel down".into()));
        app.push_plugin_stopped("a");
        assert_eq!(summary_of(&app, "a"), (Ok, "Waiting for its tunnel".into()));
        // 7: call alerts owner.
        app.plugins.get_mut("a").unwrap().offers = vec![registry::CALL_ALERTS];
        app.plugins.get_mut("a").unwrap().listening = vec!["u1.t".into()];
        assert_eq!(summary_of(&app, "a"), (Ok, "Listening on 1 account".into()));
        app.plugins.get_mut("a").unwrap().listening = vec!["u1.t".into(), "u2.t".into()];
        assert_eq!(
            summary_of(&app, "a"),
            (Ok, "Listening on 2 accounts".into())
        );
        let state = app.plugins.get_mut("a").unwrap();
        state.listening.clear();
        state.sign_in.push(PluginSignIn {
            plugin: "a".into(),
            username: None,
            account: None,
            verification_uri: "https://x".into(),
            user_code: "C".into(),
        });
        assert_eq!(
            summary_of(&app, "a"),
            (Ok, "Waiting for sign-in on 1".into())
        );
        // 6: enabled, not running yet.
        app.plugins.get_mut("a").unwrap().running = false;
        assert_eq!(summary_of(&app, "a"), (Off, "Starting\u{2026}".into()));
        // 4: stopped with a reason.
        app.plugins.get_mut("a").unwrap().message = Some("The file could not be run".into());
        assert_eq!(
            summary_of(&app, "a"),
            (Error, "Stopped: The file could not be run".into())
        );
        // 5: turned off beats starting and stopped, not the rules above.
        app.settings.plugins[0].enabled = false;
        assert_eq!(summary_of(&app, "a"), (Off, "Off".into()));
        app.settings.plugins[0].enabled = true;
        // 3: a required option unset beats stopped.
        app.plugins.get_mut("a").unwrap().options = vec![OptionSpec {
            required: true,
            ..spec("name", OptionKind::Text)
        }];
        assert_eq!(summary_of(&app, "a"), (Attention, "Needs setup".into()));
        // 2: a pending need beats an unset option.
        app.plugins.get_mut("a").unwrap().pending_needs = vec!["accounts", "push"];
        assert_eq!(
            summary_of(&app, "a"),
            (Attention, "Asks for your accounts".into())
        );
        app.plugins.get_mut("a").unwrap().pending_needs = vec!["presence"];
        assert_eq!(
            summary_of(&app, "a"),
            (Attention, "Asks for your presence".into())
        );
        app.plugins.get_mut("a").unwrap().pending_needs = vec!["push"];
        assert_eq!(
            summary_of(&app, "a"),
            (Attention, "Asks for your update notifications".into())
        );
        // 1: a changed file beats everything.
        app.plugins.get_mut("a").unwrap().file_changed = Some("h".into());
        assert_eq!(
            summary_of(&app, "a"),
            (Attention, "The file changed. Review it".into())
        );
        assert!(app.plugin_summary("a").needs_you());
        assert!(!app.plugin_summary("b").needs_you());
        // A plugin in the add flow that stopped before ready.
        app.settings.plugins[1].setup_pending = true;
        app.plugins.get_mut("b").unwrap().ready_seen = false;
        assert_eq!(summary_of(&app, "b"), (Attention, "Finish setup".into()));
        app.plugins.get_mut("b").unwrap().message = Some("x".into());
        assert_eq!(
            summary_of(&app, "b"),
            (Error, "It stopped before saying what it does".into())
        );
    }

    #[test]
    fn a_stored_choice_survives_a_restart_until_options_update() {
        let (mut app, mut commands) = option_app();
        app.sync_plugins();
        app.settings.plugins[0]
            .options
            .insert("bulb".into(), json!("old-bulb"));
        // The new run does not list it (yet): it is kept and still sent.
        ready_with(&mut app, vec![choice_spec("bulb", &["a"])]);
        assert_eq!(app.settings.plugins[0].options["bulb"], json!("old-bulb"));
        assert_eq!(
            sent_values(&mut commands).unwrap()["bulb"],
            json!("old-bulb")
        );
        let update = |values: &[&str]| PluginEvent::OptionsUpdate {
            id: "bulb".into(),
            choices: values
                .iter()
                .map(|v| OptionChoice {
                    value: (*v).into(),
                    label: (*v).into(),
                })
                .collect(),
        };
        plugin_event(&mut app, update(&["b"]));
        assert_eq!(app.settings.plugins[0].options["bulb"], json!("old-bulb"));
        // Choosing it by hand still needs it to be offered.
        app.apply_plugin_action(Action::SetPluginOption {
            plugin: "p".into(),
            option: "bulb".into(),
            value: json!("zzz"),
        });
        assert_eq!(app.settings.plugins[0].options["bulb"], json!("old-bulb"));
        // A malformed stored value still goes.
        app.settings.plugins[0]
            .options
            .insert("bulb".into(), json!("x".repeat(65)));
        ready_with(&mut app, vec![choice_spec("bulb", &["a"])]);
        assert!(!app.settings.plugins[0].options.contains_key("bulb"));
    }

    #[test]
    fn secrets_are_checked_and_trimmed_before_saving() {
        let (mut app, mut commands) = option_app();
        ready_with(
            &mut app,
            vec![
                spec("key", OptionKind::Secret),
                spec("note", OptionKind::Text),
            ],
        );
        drain(&mut commands);
        let set = |app: &mut App, option: &str, value: &str| {
            app.apply_plugin_action(Action::SetPluginSecret {
                plugin: "p".into(),
                option: option.into(),
                value: Some(value.to_owned()),
            });
        };
        set(&mut app, "key", "  k3y \n");
        let saved: Vec<_> = drain(&mut commands)
            .into_iter()
            .filter_map(|c| match c {
                Command::SavePluginSecret { value, .. } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(saved, [Some("k3y".to_owned())]);
        let key = ("p".to_owned(), "key".to_owned());
        for bad in ["   ", "a\nb", "a\u{202E}b", &"x".repeat(1001)] {
            set(&mut app, "key", bad);
            assert!(drain(&mut commands).is_empty(), "{bad:?} not saved");
            assert!(app.plugin_option_errors.contains_key(&key));
        }
        set(&mut app, "note", "v");
        assert!(drain(&mut commands).is_empty(), "not a secret");
        set(&mut app, "missing", "v");
        assert!(drain(&mut commands).is_empty());
    }
}
