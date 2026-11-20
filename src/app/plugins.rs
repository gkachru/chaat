//! The plugin list at run time: which plugins run, what they said about
//! themselves, the accounts and presence they were told, and their sign-in
//! cards. Plugins are process-wide, so this state lives on `App`, not on an
//! `Account`. The processes run on the service backend (`plugin::host`).

use std::path::PathBuf;

use super::App;
use crate::backend::Command;
use crate::model::{Action, PluginSignIn, ToastKind};
use crate::plugin::event::PluginEvent;
use crate::plugin::presence;
use crate::plugin::protocol::{HostMessage, OptionKind, OptionSpec, PluginAccount, PluginInfo};
use crate::plugin::registry;
use crate::settings::{PluginEntry, plugin_id_for};

/// The shortest time between two toasts from one plugin.
const PLUGIN_TOAST_GAP: std::time::Duration = std::time::Duration::from_secs(10);

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
    pub(super) ready_seen: bool,
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
    }
}

impl App {
    /// Sends to the service backend, except in the demo.
    fn send_service(&self, command: Command) {
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
    fn plugin_need_effective(entry: &PluginEntry, need: &str) -> bool {
        entry.needs.iter().any(|n| n == need) && entry.approved_needs.iter().any(|n| n == need)
    }

    fn plugin_needs_accounts(entry: &PluginEntry) -> bool {
        Self::plugin_need_effective(entry, "accounts")
    }

    /// What a plugin receives from Chaat now, for Settings › Plugins:
    /// `accounts` (account IDs and sign-in names), `presence` (availability
    /// and activity) and `options` (its own option values).
    pub fn plugin_receives(&self, id: &str) -> Vec<&'static str> {
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return Vec::new();
        };
        let state = self.plugins.get(id);
        let mut receives = Vec::new();
        for need in ["accounts", "presence"] {
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
        if self.demo {
            return;
        }
        let wanted: Vec<(String, PathBuf)> = self
            .settings
            .plugins
            .iter()
            .filter(|entry| entry.enabled && !entry.path.as_os_str().is_empty())
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
            self.retire_plugin_generation(&id);
            self.service.send(Command::StopPlugin { id: id.clone() });
            if let Some(state) = self.plugins.get_mut(&id) {
                state.stopped();
            }
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
        let mut values = std::collections::BTreeMap::new();
        let mut secrets = Vec::new();
        for spec in &state.options {
            if spec.kind == OptionKind::Secret {
                if entry.secrets.contains(&spec.id) && !secrets.contains(&spec.id) {
                    secrets.push(spec.id.clone());
                }
            } else if let Some(value) = entry.options.get(&spec.id).or(spec.default.as_ref()) {
                values.insert(spec.id.clone(), value.clone());
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
                }
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
            // C2: record the status and the new choices.
            PluginEvent::OptionStatus { .. } | PluginEvent::OptionsUpdate { .. } => {}
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
            entry
                .options
                .retain(|option, _| !now_secret.contains(option));
            if entry.options.len() != before {
                self.mark_settings_dirty();
            }
        }
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
    }

    /// Adds a plugin to the list, turned on, and starts it.
    pub(super) fn add_plugin(&mut self, path: PathBuf) {
        let taken: Vec<String> = self.settings.plugins.iter().map(|e| e.id.clone()).collect();
        let id = plugin_id_for(&path, &taken);
        self.settings.plugins.push(PluginEntry {
            id,
            path,
            enabled: true,
            instance: PluginEntry::new_instance(),
            ..PluginEntry::default()
        });
        self.mark_settings_dirty();
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
        if self.plugins_started.remove(id).is_some() {
            self.retire_plugin_generation(id);
            self.send_service(Command::StopPlugin { id: id.to_owned() });
        }
        self.settings.plugins.retain(|e| e.id != id);
        self.plugins.remove(id);
        self.mark_settings_dirty();
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
                if let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == plugin) {
                    entry.options.insert(option, value);
                    self.mark_settings_dirty();
                    self.send_plugin_options(&plugin);
                }
            }
            Action::SetPluginSecret {
                plugin,
                option,
                value,
            } => {
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

    /// Allows or refuses what the plugin's current `ready` asked for and the
    /// person has not decided yet. Allowed accounts and presence go out now.
    fn decide_plugin_needs(&mut self, id: &str, allow: bool) {
        let pending = match self.plugins.get_mut(id) {
            Some(state) if !state.pending_needs.is_empty() => {
                std::mem::take(&mut state.pending_needs)
            }
            _ => return,
        };
        let Some(entry) = self.settings.plugins.iter_mut().find(|e| e.id == id) else {
            return;
        };
        for need in pending {
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
        if !allow {
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
}
