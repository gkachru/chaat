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
    /// Sign-ins it is waiting for, one per account.
    pub sign_in: Vec<PluginSignIn>,
    /// Accounts (`oid.tid`) it said it listens for.
    pub listening: Vec<String>,
    /// The last notice about it (why it stopped), for the status line.
    pub message: Option<String>,
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

    fn plugin_needs_accounts(entry: &PluginEntry) -> bool {
        entry.needs.iter().any(|need| need == "accounts")
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
            forget: entry.forget.clone(),
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
            self.plugins_started.insert(id.clone(), path.clone());
            let state = self.plugins.entry(id.clone()).or_default();
            state.stopped();
            state.sent_accounts = accounts;
            self.service.send(Command::StartPlugin { id, path, hello });
        }
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
            let forget = entry.forget.clone();
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
            if state.running
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

    /// Sends a plugin its option values: the stored ones, the default of each
    /// unset non-secret option, and the names of its stored secrets (the
    /// worker adds their values).
    pub(super) fn send_plugin_options(&self, id: &str) {
        let Some(entry) = self.settings.plugins.iter().find(|e| e.id == id) else {
            return;
        };
        let mut values = entry.options.clone();
        if let Some(state) = self.plugins.get(id) {
            for spec in &state.options {
                if spec.kind != OptionKind::Secret
                    && !values.contains_key(&spec.id)
                    && let Some(default) = &spec.default
                {
                    values.insert(spec.id.clone(), default.clone());
                }
            }
        }
        self.send_service(Command::PluginOptions {
            id: id.to_owned(),
            values,
            secrets: entry.secrets.clone(),
        });
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
    pub(super) fn on_plugin_event(&mut self, id: String, event: PluginEvent) {
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
                    let name = self.plugin_name(&id);
                    self.toast(ToastKind::Error, format!("{name}: {message}"));
                }
            }
            PluginEvent::SignIn {
                account,
                verification_uri,
                user_code,
            } => {
                if !self.plugin_counts(&id) {
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
                let state = self.plugins.entry(id.clone()).or_default();
                state.sign_in.retain(|s| s.account != account);
                state.asked.retain(|asked| *asked != account);
                let name = self.plugin_name(&id);
                let text = match self.plugin_username(account.as_deref()) {
                    Some(user) if self.accounts.len() > 1 => {
                        format!("{name}: signed in for {user}.")
                    }
                    _ => format!("{name}: signed in."),
                };
                self.toast(ToastKind::Info, text);
            }
            PluginEvent::Listening { account } => {
                let state = self.plugins.entry(id).or_default();
                if !state.listening.contains(&account) {
                    state.listening.push(account);
                }
            }
            PluginEvent::Error { account, message } => {
                if let Some(account) = &account {
                    let state = self.plugins.entry(id.clone()).or_default();
                    state.listening.retain(|a| a != account);
                }
                let name = self.plugin_name(&id);
                let text = match self.plugin_username(account.as_deref()) {
                    Some(user) => format!("{name} for {user}: {message}"),
                    None => format!("{name}: {message}"),
                };
                self.toast(ToastKind::Error, text);
            }
            PluginEvent::IncomingCall(alert) => {
                if self.call_alerts_owner().as_deref() == Some(id.as_str()) {
                    self.on_incoming_call(alert);
                }
            }
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
        let wants_accounts = needs.contains(&"accounts");
        let state = self.plugins.entry(id.to_owned()).or_default();
        state.running = true;
        state.message = None;
        state.info = info;
        state.offers = offers;
        state.needs = needs.clone();
        state.options = options;
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
        for option in &entry.secrets {
            self.send_service(Command::SavePluginSecret {
                plugin: id.to_owned(),
                option: option.clone(),
                value: None,
            });
        }
        if self.plugins_started.remove(id).is_some() {
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
            } => self.send_service(Command::SavePluginSecret {
                plugin,
                option,
                value,
            }),
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

    /// Records whether a plugin secret was stored.
    pub(super) fn on_plugin_secret_saved(
        &mut self,
        plugin: &str,
        option: String,
        set: bool,
        ok: bool,
    ) {
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
