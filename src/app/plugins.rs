//! The plugin list at run time: which plugins run, what they said about
//! themselves, the accounts and presence they were told, and their sign-in
//! cards. Plugins are process-wide, so this state lives on `App`, not on an
//! `Account`. The processes run on the service backend (`plugin::host`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::App;
use super::PushPluginState;
use crate::backend::ServiceCommand;
use crate::model::{PluginAction, PluginSignIn, SettingsPage, ToastKind};
use crate::plugin::event::PluginEvent;
use crate::plugin::options;
use crate::plugin::presence;
use crate::plugin::protocol::{
    HostMessage, OptionKind, OptionSpec, PluginAccount, PluginInfo, StatusState,
};
use crate::plugin::registry::{self, Capability, Need};
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
    pub(super) fn send_service(&self, command: ServiceCommand) {
        if !self.demo {
            self.service.send(command);
        }
    }

    /// The need a message to a plugin depends on. Accounts need `accounts`,
    /// presence `presence`, a non-empty channel list `push` and an agent event
    /// `messages`. Everything else carries nothing the person has to allow
    /// (an empty channel list tells the plugin to accept nothing, a result
    /// answers the plugin's own request) and is never held back.
    fn message_need(message: &HostMessage) -> Option<Need> {
        match message {
            HostMessage::Accounts { .. } => Some(Need::Accounts),
            HostMessage::Presence { .. } => Some(Need::Presence),
            HostMessage::PushChannels { channels } if !channels.is_empty() => Some(Need::Push),
            HostMessage::AgentEvent { .. } => Some(Need::Messages),
            _ => None,
        }
    }

    /// Whether the gate in [`App::send_to_plugin`] lets `message` through to
    /// plugin `id`.
    fn plugin_may_receive(&self, id: &str, message: &HostMessage) -> bool {
        let Some(need) = Self::message_need(message) else {
            return true;
        };
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return false;
        };
        if !Self::plugin_need_effective(entry, need) {
            return false;
        }
        let running = self.plugins.get(id).is_some_and(|state| state.running);
        need == Need::Accounts || (entry.enabled && running)
    }

    /// Sends a message to a plugin, the one place `Command::PluginSend` is
    /// built. A message that depends on a need goes only if the plugin
    /// declared it and the person allowed it, and (except accounts, which the
    /// plugin may be sent while it starts) only to a plugin that is enabled
    /// and running. The gate can only drop a message.
    pub(super) fn send_to_plugin(&self, id: &str, message: HostMessage) {
        if !self.plugin_may_receive(id, &message) {
            return;
        }
        self.send_service(ServiceCommand::PluginSend {
            id: id.to_owned(),
            message,
        });
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
        self.account_index(account)
            .map(|index| self.accounts[index].username())
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
        self.owner_of(Capability::CallAlerts)
    }

    /// The first enabled plugin in list order that is running and offers
    /// `capability`.
    pub fn owner_of(&self, capability: Capability) -> Option<String> {
        let name = capability.as_str();
        self.settings
            .plugins
            .iter()
            .filter(|entry| entry.enabled)
            .find(|entry| {
                self.plugins
                    .get(&entry.id)
                    .is_some_and(|state| state.running && state.offers.contains(&name))
            })
            .map(|entry| entry.id.clone())
    }

    /// Whether the plugin declared `need` in its last `ready` and the person
    /// allowed it. Only such a need decides what Chaat sends.
    pub(super) fn plugin_need_effective(entry: &PluginEntry, need: Need) -> bool {
        let name = need.as_str();
        entry.needs.iter().any(|n| n == name) && entry.approved_needs.iter().any(|n| n == name)
    }

    fn plugin_needs_accounts(entry: &PluginEntry) -> bool {
        Self::plugin_need_effective(entry, Need::Accounts)
    }

    /// What a plugin receives from Chaat now, for Settings › Plugins:
    /// `accounts` (account IDs and sign-in names), `presence` (availability
    /// and activity), `push` (update channels, no account ids), `messages` (message text in
    /// the chats the person allows) and `options`
    /// (its own option values).
    pub fn plugin_receives(&self, id: &str) -> Vec<&'static str> {
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return Vec::new();
        };
        let state = self.plugins.get(id);
        let mut receives = Vec::new();
        for need in Need::ALL {
            if Self::plugin_need_effective(entry, need) {
                receives.push(need.as_str());
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
            self.service
                .send(ServiceCommand::StopPlugin { id: id.clone() });
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
            self.service.send(ServiceCommand::StartPlugin {
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
        let message = HostMessage::Accounts {
            accounts: accounts.clone(),
        };
        // The restart hello must not carry accounts the gate holds back.
        if !self.plugin_may_receive(id, &message) {
            return;
        }
        self.send_to_plugin(id, message);
        self.send_service(ServiceCommand::PluginHello {
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
            self.send_service(ServiceCommand::PluginHello {
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
                self.send_to_plugin(
                    &id,
                    HostMessage::Forget {
                        account: account.to_owned(),
                    },
                );
                self.send_service(ServiceCommand::PluginHello {
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
            let allowed =
                self.settings.plugins.iter().any(|entry| {
                    entry.id == *id && Self::plugin_need_effective(entry, Need::Presence)
                });
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
            self.send_to_plugin(&id, message.clone());
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
        self.send_service(ServiceCommand::PluginOptions {
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
                self.prune_agent_proposals();
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
            PluginEvent::AgentQuery {
                request,
                account,
                query,
            } => self.on_agent_query(&id, request, account, query),
            PluginEvent::AgentPropose {
                request,
                account,
                proposal,
            } => self.on_agent_propose(&id, request, account, proposal),
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
            self.send_service(ServiceCommand::PluginHello {
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
                self.send_to_plugin(
                    id,
                    HostMessage::Forget {
                        account: account.id,
                    },
                );
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
            self.send_service(ServiceCommand::SavePluginSecret {
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
            self.send_service(ServiceCommand::StopPlugin { id: id.to_owned() });
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
            self.send_service(ServiceCommand::StopPlugin { id: id.to_owned() });
        }
        self.plugin_started_at.remove(id);
        if let Some(state) = self.plugins.get_mut(id) {
            state.message = None;
        }
        self.sync_plugins();
    }

    /// Runs the plugin actions.
    pub(super) fn apply_plugin_action(&mut self, action: PluginAction) {
        match action {
            PluginAction::Add => self.choose_plugin(),
            PluginAction::Remove(id) => self.remove_plugin(&id),
            PluginAction::SetEnabled(id, enabled) => {
                if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) {
                    entry.enabled = enabled;
                    self.mark_settings_dirty();
                    self.sync_plugins();
                }
            }
            PluginAction::SetOption {
                plugin,
                option,
                value,
            } => {
                self.set_plugin_option(plugin, option, value);
            }
            PluginAction::RunAction { plugin, action } => self.run_plugin_action(plugin, action),
            PluginAction::ResetSection { plugin, section } => {
                self.reset_plugin_section(&plugin, section.as_deref());
            }
            PluginAction::Open(id) => self.open_plugin(id),
            PluginAction::BackToPlugins => {
                self.plugins_view = PluginsView::List;
                self.sync_plugins();
            }
            PluginAction::StartReorder => {
                if self.settings.plugins.len() >= 2 {
                    self.reordering = true;
                    self.plugins_view = PluginsView::List;
                    self.sync_plugins();
                }
            }
            PluginAction::EndReorder => self.reordering = false,
            PluginAction::Move { id, to } => self.move_plugin(&id, to),
            PluginAction::FinishSetup { id, allow } => self.finish_plugin_setup(&id, &allow),
            PluginAction::CancelSetup(id) => {
                if self
                    .settings
                    .plugins
                    .iter()
                    .any(|e| e.id == id && e.setup_pending)
                {
                    self.remove_plugin(&id);
                }
            }
            PluginAction::RetryStart(id) => self.retry_plugin_start(&id),
            PluginAction::SetSecret {
                plugin,
                option,
                value,
            } => self.set_plugin_secret(plugin, option, value),
            PluginAction::ApproveNeeds(id) => self.decide_plugin_needs(&id, true),
            PluginAction::DenyNeeds(id) => self.decide_plugin_needs(&id, false),
            PluginAction::TrustFile(id) => self.trust_plugin_file(&id),
            PluginAction::KeepOff(id) => {
                if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) {
                    entry.enabled = false;
                    self.mark_settings_dirty();
                }
                if let Some(state) = self.plugins.get_mut(&id) {
                    state.file_changed = None;
                }
                self.sync_plugins();
            }
            PluginAction::SignIn { plugin, account } => {
                self.send_to_plugin(&plugin, HostMessage::SignIn { account })
            }
            PluginAction::HideSignIn { plugin, account } => {
                if let Some(state) = self.plugins.get_mut(&plugin) {
                    state.sign_in.retain(|s| s.account != account);
                }
            }
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
            self.send_service(ServiceCommand::SavePluginSecret {
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
        self.send_to_plugin(&plugin, HostMessage::Action { id: action });
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
                self.send_service(ServiceCommand::PluginHello {
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
        "messages" => "messages",
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
        if self.agents_active().as_deref() == Some(id) {
            // Another plugin takes over only if it may read messages too.
            let other = self
                .plugin_next_provider(id, registry::AGENTS)
                .filter(|other| {
                    self.settings.plugins.iter().any(|entry| {
                        entry.id == *other && Self::plugin_need_effective(entry, Need::Messages)
                    })
                });
            sentences.push(match other {
                Some(other) => format!(
                    "{} reads your allowed chats and suggests replies instead.",
                    self.quoted_plugin_name(&other)
                ),
                None => "Agents stop reading your allowed chats and suggesting replies. \
                         Your allow lists are kept."
                    .to_owned(),
            });
            match self.agent_proposals.count_from(id) {
                0 => {}
                1 => sentences.push("1 pending suggestion is discarded.".to_owned()),
                n => sentences.push(format!("{n} pending suggestions are discarded.")),
            }
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
mod tests;
