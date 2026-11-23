//! Push updates through a plugin (docs/realtime.md): which plugin receives
//! Graph's change notifications, the channel each account listens on, and
//! the route each account backend is given. Chaat checks every line the
//! plugin sends; bodies, channels and addresses are never logged.

use super::App;
use crate::backend::{Command, PushRoute};
use crate::plugin::event::PushEvent;
use crate::plugin::protocol::HostMessage;
use crate::plugin::registry::{self, Capability, Need};

/// What Settings › Push updates shows about the push plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushPluginView {
    /// The plugin's cleaned name, else its id.
    pub name: String,
    pub state: PushPluginState,
    /// Whether a manual relay address is set; it wins over the plugin.
    pub relay_set: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushPluginState {
    /// The plugin has not said its tunnel is up.
    Waiting,
    /// The tunnel is up at this host.
    Up { host: String },
    /// The plugin said the tunnel is down, with its plain-text reason.
    Down { reason: Option<String> },
    /// The plugin asked for the `push` need and the person has not allowed it.
    NeedsApproval,
}

/// What the app knows about the push plugin's route, for the current run of
/// the plugin only.
#[derive(Debug, Default)]
pub(super) struct PushRouting {
    /// The address the plugin last announced: plugin id, the generation of
    /// the run that said it, and the origin.
    /// Kept for every push-offering plugin, so one that takes over from a
    /// stopped owner already has its address.
    origins: std::collections::BTreeMap<String, (u64, String)>,
    /// The plain-text reason of each plugin's last `push_down`.
    downs: std::collections::BTreeMap<String, Option<String>>,
    /// The plugin id and origin the account backends were last given.
    active: Option<(String, String)>,
    /// The plugin id and channel list it was last told.
    announced: Option<(String, Vec<String>)>,
}

impl App {
    /// The plugin that receives pushes: the first enabled one in list order
    /// that is running and offers `push`.
    pub fn push_owner(&self) -> Option<String> {
        self.owner_of(Capability::Push)
    }

    /// Whether the plugin declared `push` and the person allowed it.
    fn push_allowed(&self, id: &str) -> bool {
        self.settings
            .plugins
            .iter()
            .find(|entry| entry.id == id)
            .is_some_and(|entry| Self::plugin_need_effective(entry, Need::Push))
    }

    /// What Settings › Push updates shows, or `None` when no enabled plugin
    /// offers `push`.
    pub fn push_plugin_view(&self) -> Option<PushPluginView> {
        let id = self.push_owner().or_else(|| {
            self.settings
                .plugins
                .iter()
                .filter(|entry| entry.enabled)
                .find(|entry| {
                    self.plugins
                        .get(&entry.id)
                        .is_some_and(|state| state.offers.contains(&registry::PUSH))
                })
                .map(|entry| entry.id.clone())
        })?;
        let declared = self
            .plugins
            .get(&id)
            .is_some_and(|state| state.needs.contains(&registry::PUSH));
        let state = if declared && !self.push_allowed(&id) {
            PushPluginState::NeedsApproval
        } else if let Some((generation, url)) = self.push_plugin.origins.get(&id)
            && self.push_generation_current(&id, *generation)
            && self.push_owner().as_deref() == Some(id.as_str())
        {
            PushPluginState::Up {
                host: url.strip_prefix("https://").unwrap_or(url).to_owned(),
            }
        } else if let Some(reason) = self.push_plugin.downs.get(&id) {
            PushPluginState::Down {
                reason: reason.clone(),
            }
        } else {
            PushPluginState::Waiting
        };
        Some(PushPluginView {
            name: self.plugin_name(&id),
            state,
            relay_set: self.settings.relay_url.is_some(),
        })
    }

    fn push_generation_current(&self, id: &str, generation: u64) -> bool {
        self.plugin_generations
            .get(id)
            .is_none_or(|current| *current == generation)
    }

    /// The plugin and origin the accounts should use now, if any: no manual
    /// relay, the plugin owns `push`, is allowed it, and its tunnel is up in
    /// the current run.
    fn push_desired(&self) -> Option<(String, String)> {
        if self.settings.relay_url.is_some() {
            return None;
        }
        let owner = self.push_owner()?;
        let (generation, url) = self.push_plugin.origins.get(&owner)?;
        (self.push_allowed(&owner) && self.push_generation_current(&owner, *generation))
            .then(|| (owner, url.clone()))
    }

    /// Makes the account backends and the plugin match what should be: the
    /// channel list first, then each account's route. With `resend`, a route
    /// that is already up is announced and given again (a new `push_ready`,
    /// which can carry a new address).
    pub(super) fn sync_push_route(&mut self, resend: bool) {
        if self.demo {
            return;
        }
        let desired = self.push_desired();
        let channels: Vec<String> = self
            .accounts
            .iter()
            .map(|account| account.push_channel.clone())
            .collect();
        match &desired {
            Some((id, url)) => {
                // A plugin that had the list and no longer owns push is told
                // to accept nothing.
                if let Some((old, list)) = &self.push_plugin.announced
                    && old != id
                    && !list.is_empty()
                    && self.plugins.get(old).is_some_and(|state| state.running)
                {
                    self.send_to_plugin(
                        old,
                        HostMessage::PushChannels {
                            channels: Vec::new(),
                        },
                    );
                    self.push_plugin.announced = None;
                }
                let told =
                    self.push_plugin.announced.as_ref() == Some(&(id.clone(), channels.clone()));
                if resend || !told {
                    self.send_to_plugin(
                        id,
                        HostMessage::PushChannels {
                            channels: channels.clone(),
                        },
                    );
                    self.push_plugin.announced = Some((id.clone(), channels));
                }
                if resend || self.push_plugin.active != desired {
                    for account in &self.accounts {
                        account
                            .backend
                            .send(Command::PushPlugin(Some(PushRoute::new(
                                url.clone(),
                                account.push_channel.clone(),
                            ))));
                    }
                }
            }
            None => {
                if self.push_plugin.active.is_some() {
                    for account in &self.accounts {
                        account.backend.send(Command::PushPlugin(None));
                    }
                }
                // A running plugin is told to accept nothing; one that
                // stopped has forgotten the list already.
                if let Some((id, list)) = self.push_plugin.announced.take()
                    && !list.is_empty()
                    && self.plugins_started.contains_key(&id)
                    && self.plugins.get(&id).is_some_and(|state| state.running)
                {
                    self.send_to_plugin(
                        &id,
                        HostMessage::PushChannels {
                            channels: Vec::new(),
                        },
                    );
                }
            }
        }
        self.push_plugin.active = desired;
    }

    /// The route a newly started account backend is given, if one is up.
    pub(super) fn push_route_for(&self, index: usize) -> Option<Command> {
        if self.demo {
            return None;
        }
        let (_, url) = self.push_plugin.active.as_ref()?;
        let account = self.accounts.get(index)?;
        Some(Command::PushPlugin(Some(PushRoute::new(
            url.clone(),
            account.push_channel.clone(),
        ))))
    }

    /// Forgets the address and state of a plugin run that ended.
    pub(super) fn push_plugin_stopped(&mut self, id: &str) {
        self.push_plugin.origins.remove(id);
        self.push_plugin.downs.remove(id);
    }

    /// Whether the plugin is enabled, running and offers `push`.
    fn push_offered_running(&self, id: &str) -> bool {
        self.settings
            .plugins
            .iter()
            .any(|entry| entry.id == id && entry.enabled)
            && self
                .plugins
                .get(id)
                .is_some_and(|state| state.running && state.offers.contains(&registry::PUSH))
    }

    /// Applies a push message from a plugin. Only the owner is heard; its
    /// address is remembered even before the person allows `push`, so
    /// allowing it later starts the route.
    pub(super) fn on_push_event(&mut self, id: &str, generation: u64, event: PushEvent) {
        if self.push_owner().as_deref() != Some(id) {
            // A second push plugin is not acted on, but its address is kept
            // for when it takes over.
            if self.push_offered_running(id) {
                match event {
                    PushEvent::Ready { url } => {
                        self.push_plugin
                            .origins
                            .insert(id.to_owned(), (generation, url));
                        self.push_plugin.downs.remove(id);
                    }
                    PushEvent::Down { reason } => {
                        self.push_plugin.origins.remove(id);
                        self.push_plugin.downs.insert(id.to_owned(), reason);
                    }
                    PushEvent::Body { .. } => {}
                }
            }
            return;
        }
        match event {
            PushEvent::Ready { url } => {
                let repeat = self
                    .push_plugin
                    .origins
                    .insert(id.to_owned(), (generation, url.clone()))
                    == Some((generation, url));
                self.push_plugin.downs.remove(id);
                if !self.push_allowed(id) {
                    return;
                }
                // The same address from the same run while the route is up
                // changes nothing: do not tear the subscriptions down again.
                if repeat
                    && self.push_plugin.active.is_some()
                    && self.push_plugin.active == self.push_desired()
                {
                    return;
                }
                if self.settings.relay_url.is_some() {
                    // The relay wins: the plugin accepts nothing.
                    self.send_to_plugin(
                        id,
                        HostMessage::PushChannels {
                            channels: Vec::new(),
                        },
                    );
                    self.push_plugin.announced = None;
                }
                self.sync_push_route(true);
            }
            PushEvent::Down { reason } => {
                self.push_plugin.origins.remove(id);
                self.push_plugin.downs.insert(id.to_owned(), reason);
                self.sync_push_route(false);
            }
            PushEvent::Body { channel, body } => {
                if !self.push_allowed(id) || self.push_plugin.active.is_none() {
                    return;
                }
                if let Some(account) = self
                    .accounts
                    .iter()
                    .find(|account| account.push_channel == channel)
                {
                    account.backend.send(Command::PushBody(body));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Account, PluginState};
    use crate::backend::{Backend, Waker};
    use crate::model::{Action, PluginAction};
    use crate::paths::AppDirs;
    use crate::plugin::event::PluginEvent;
    use crate::settings::{PluginEntry, Settings};
    use tokio::sync::mpsc::UnboundedReceiver;

    type Rx = UnboundedReceiver<Command>;

    /// `Command` has no equality; its debug text stands in.
    fn shown(commands: &[Command]) -> Vec<String> {
        commands.iter().map(|c| format!("{c:?}")).collect()
    }

    fn same(got: Vec<Command>, want: Vec<Command>) {
        assert_eq!(shown(&got), shown(&want));
    }

    fn has(got: Vec<Command>, want: Command) -> bool {
        shown(&got).contains(&format!("{want:?}"))
    }

    fn drain(rx: &mut Rx) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// An app with two accounts and a running push plugin "pr" that the
    /// person allowed (when `approved`). Returns the service, first account
    /// and second account command receivers.
    fn setup(approved: bool) -> (App, Rx, Rx, Rx) {
        let (backend, mut first) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-unused");
        let mut app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            false,
        );
        let (service, service_rx) = Backend::recording();
        app.service = service;
        let (second_backend, second) = Backend::recording();
        app.accounts.push(Account::new(second_backend));
        app.settings.plugins.push(PluginEntry {
            id: "pr".into(),
            path: "pr.exe".into(),
            enabled: true,
            needs: vec!["push".into()],
            approved_needs: if approved {
                vec!["push".into()]
            } else {
                vec![]
            },
            instance: "inst".into(),
            ..Default::default()
        });
        app.plugins.insert(
            "pr".into(),
            PluginState {
                running: true,
                offers: vec![registry::PUSH],
                needs: vec!["push"],
                pending_needs: if approved { vec![] } else { vec!["push"] },
                ..Default::default()
            },
        );
        app.plugins_started.insert("pr".into(), "pr.exe".into());
        drain(&mut first);
        (app, service_rx, first, second)
    }

    fn event(app: &mut App, id: &str, event: PushEvent) {
        let generation = app.plugin_generations.get(id).copied().unwrap_or(0);
        app.on_plugin_event(id.into(), generation, PluginEvent::Push(event));
    }

    fn ready(url: &str) -> PushEvent {
        PushEvent::Ready { url: url.into() }
    }

    const URL: &str = "https://a.trycloudflare.com";

    fn channels(app: &App) -> Vec<String> {
        app.accounts
            .iter()
            .map(|a| a.push_channel.clone())
            .collect()
    }

    fn route(url: &str, channel: &str) -> Command {
        Command::PushPlugin(Some(PushRoute::new(url.into(), channel.into())))
    }

    fn told(list: Vec<String>) -> Command {
        Command::PluginSend {
            id: "pr".into(),
            message: HostMessage::PushChannels { channels: list },
        }
    }

    #[test]
    fn accounts_get_random_channels_of_the_right_shape() {
        let (app, ..) = setup(true);
        let list = channels(&app);
        assert_eq!(list.len(), 2);
        assert_ne!(list[0], list[1]);
        assert!(
            list.iter()
                .all(|c| crate::plugin::protocol::is_push_channel(c))
        );
    }

    #[test]
    fn push_ready_announces_channels_then_routes() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        let list = channels(&app);
        same(drain(&mut service), vec![told(list.clone())]);
        same(drain(&mut first), vec![route(URL, &list[0])]);
        same(drain(&mut second), vec![route(URL, &list[1])]);
        // Another ready (a new address) announces and routes again.
        event(&mut app, "pr", ready("https://b.trycloudflare.com"));
        same(drain(&mut service), vec![told(list.clone())]);
        same(
            drain(&mut first),
            vec![route("https://b.trycloudflare.com", &list[0])],
        );
    }

    #[test]
    fn same_push_ready_twice_does_not_resubscribe() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        event(&mut app, "pr", ready(URL));
        assert!(drain(&mut service).is_empty());
        assert!(drain(&mut first).is_empty());
        assert!(drain(&mut second).is_empty());
        // After a down, the same address resubscribes.
        event(&mut app, "pr", PushEvent::Down { reason: None });
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        event(&mut app, "pr", ready(URL));
        let list = channels(&app);
        same(drain(&mut first), vec![route(URL, &list[0])]);
        same(drain(&mut second), vec![route(URL, &list[1])]);
    }

    #[test]
    fn push_body_goes_to_its_account() {
        let (mut app, _service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut first);
        drain(&mut second);
        let list = channels(&app);
        event(
            &mut app,
            "pr",
            PushEvent::Body {
                channel: list[1].clone(),
                body: "{}".into(),
            },
        );
        same(drain(&mut second), vec![Command::PushBody("{}".into())]);
        assert!(drain(&mut first).is_empty());
    }

    #[test]
    fn unknown_channel_is_dropped() {
        let (mut app, _service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut first);
        drain(&mut second);
        event(
            &mut app,
            "pr",
            PushEvent::Body {
                channel: "x".repeat(43),
                body: "{}".into(),
            },
        );
        assert!(drain(&mut first).is_empty() && drain(&mut second).is_empty());
    }

    #[test]
    fn push_from_non_owner_is_ignored() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        app.settings.plugins.push(PluginEntry {
            id: "other".into(),
            enabled: true,
            needs: vec!["push".into()],
            approved_needs: vec!["push".into()],
            ..Default::default()
        });
        app.plugins.insert(
            "other".into(),
            PluginState {
                running: true,
                offers: vec![registry::PUSH],
                needs: vec!["push"],
                ..Default::default()
            },
        );
        // "pr" is first in the list and owns push; "other" is a second one.
        event(&mut app, "other", ready(URL));
        let first_channel = channels(&app)[0].clone();
        event(
            &mut app,
            "other",
            PushEvent::Body {
                channel: first_channel,
                body: "{}".into(),
            },
        );
        assert!(drain(&mut service).is_empty());
        assert!(drain(&mut first).is_empty() && drain(&mut second).is_empty());
        // A plugin that does not offer push is not heard either.
        app.settings.plugins.push(PluginEntry {
            id: "plain".into(),
            enabled: true,
            ..Default::default()
        });
        event(&mut app, "plain", ready(URL));
        assert!(drain(&mut first).is_empty());
    }

    #[test]
    fn a_second_plugin_takes_over_when_the_owner_stops() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        app.settings.plugins.push(PluginEntry {
            id: "other".into(),
            enabled: true,
            needs: vec!["push".into()],
            approved_needs: vec!["push".into()],
            ..Default::default()
        });
        app.plugins.insert(
            "other".into(),
            PluginState {
                running: true,
                offers: vec![registry::PUSH],
                needs: vec!["push"],
                ..Default::default()
            },
        );
        app.plugins_started.insert("other".into(), "o.exe".into());
        event(&mut app, "pr", ready(URL));
        event(&mut app, "other", ready("https://b.trycloudflare.com"));
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        let generation = app.plugin_generations.get("pr").copied().unwrap_or(0);
        app.on_plugin_event(
            "pr".into(),
            generation,
            PluginEvent::Status {
                running: false,
                message: None,
            },
        );
        let list = channels(&app);
        let sent = drain(&mut service);
        assert!(has(
            sent,
            Command::PluginSend {
                id: "other".into(),
                message: HostMessage::PushChannels {
                    channels: list.clone()
                },
            }
        ));
        assert!(has(
            drain(&mut first),
            route("https://b.trycloudflare.com", &list[0])
        ));
        assert!(has(
            drain(&mut second),
            route("https://b.trycloudflare.com", &list[1])
        ));
        // The old plugin's late bodies go nowhere.
        event(
            &mut app,
            "pr",
            PushEvent::Body {
                channel: list[0].clone(),
                body: "{}".into(),
            },
        );
        assert!(drain(&mut first).is_empty());
    }

    #[test]
    fn approval_withdrawn_turns_the_route_off() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        app.settings.plugins[0].approved_needs.clear();
        app.sync_push_route(false);
        same(drain(&mut first), vec![Command::PushPlugin(None)]);
        same(drain(&mut second), vec![Command::PushPlugin(None)]);
        same(drain(&mut service), vec![told(vec![])]);
    }

    #[test]
    fn unapproved_push_ready_sends_nothing() {
        let (mut app, mut service, mut first, mut second) = setup(false);
        event(&mut app, "pr", ready(URL));
        let first_channel = channels(&app)[0].clone();
        event(
            &mut app,
            "pr",
            PushEvent::Body {
                channel: first_channel,
                body: "{}".into(),
            },
        );
        assert!(drain(&mut service).is_empty());
        assert!(drain(&mut first).is_empty() && drain(&mut second).is_empty());
        assert_eq!(
            app.push_plugin_view().unwrap().state,
            PushPluginState::NeedsApproval
        );
        // Allowing it later starts the route the plugin already announced.
        app.apply(
            Action::Plugin(PluginAction::ApproveNeeds("pr".into())),
            &egui::Context::default(),
        );
        let list = channels(&app);
        assert!(has(drain(&mut service), told(list.clone())));
        same(drain(&mut first), vec![route(URL, &list[0])]);
        same(drain(&mut second), vec![route(URL, &list[1])]);
    }

    #[test]
    fn refusing_the_need_keeps_the_route_off() {
        let (mut app, mut service, mut first, _second) = setup(false);
        event(&mut app, "pr", ready(URL));
        app.apply(
            Action::Plugin(PluginAction::DenyNeeds("pr".into())),
            &egui::Context::default(),
        );
        assert!(drain(&mut service).is_empty() && drain(&mut first).is_empty());
        assert_eq!(
            app.push_plugin_view().unwrap().state,
            PushPluginState::NeedsApproval
        );
    }

    #[test]
    fn push_down_turns_routes_off() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        event(
            &mut app,
            "pr",
            PushEvent::Down {
                reason: Some("cloudflared stopped".into()),
            },
        );
        same(drain(&mut first), vec![Command::PushPlugin(None)]);
        same(drain(&mut second), vec![Command::PushPlugin(None)]);
        same(drain(&mut service), vec![told(vec![])]);
        assert_eq!(
            app.push_plugin_view().unwrap().state,
            PushPluginState::Down {
                reason: Some("cloudflared stopped".into())
            }
        );
        // Bodies after the route went down are dropped.
        let first_channel = channels(&app)[0].clone();
        event(
            &mut app,
            "pr",
            PushEvent::Body {
                channel: first_channel,
                body: "{}".into(),
            },
        );
        assert!(drain(&mut first).is_empty());
    }

    #[test]
    fn stop_sends_none() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        let generation = app.plugin_generations.get("pr").copied().unwrap_or(0);
        app.on_plugin_event(
            "pr".into(),
            generation,
            PluginEvent::Status {
                running: false,
                message: None,
            },
        );
        same(drain(&mut first), vec![Command::PushPlugin(None)]);
        same(drain(&mut second), vec![Command::PushPlugin(None)]);
        // The stopped plugin forgot the list; nothing is sent to it.
        assert!(drain(&mut service).is_empty());
        assert_eq!(
            app.push_plugin_view().unwrap().state,
            PushPluginState::Waiting
        );
    }

    #[test]
    fn turning_off_and_removing_send_none() {
        let ctx = egui::Context::default();
        let (mut app, mut service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        app.apply(
            Action::Plugin(PluginAction::SetEnabled("pr".into(), false)),
            &ctx,
        );
        same(drain(&mut first), vec![Command::PushPlugin(None)]);
        same(drain(&mut second), vec![Command::PushPlugin(None)]);
        assert!(
            !drain(&mut service).iter().any(|c| matches!(
                c,
                Command::PluginSend {
                    message: HostMessage::PushChannels { .. },
                    ..
                }
            )),
            "a plugin being stopped is not told anything"
        );

        let (mut app, _service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut first);
        drain(&mut second);
        app.apply(Action::Plugin(PluginAction::Remove("pr".into())), &ctx);
        same(drain(&mut first), vec![Command::PushPlugin(None)]);
        same(drain(&mut second), vec![Command::PushPlugin(None)]);
        assert!(app.push_plugin_view().is_none());
    }

    #[test]
    fn relay_url_wins() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        app.settings.relay_url = Some("https://relay.example.com".into());
        event(&mut app, "pr", ready(URL));
        assert!(drain(&mut first).is_empty() && drain(&mut second).is_empty());
        same(drain(&mut service), vec![told(vec![])]);
        assert!(app.push_plugin_view().unwrap().relay_set);
        // Clearing the relay brings the plugin back, channels first.
        app.apply(Action::SetRelay(String::new()), &egui::Context::default());
        let list = channels(&app);
        same(drain(&mut service), vec![told(list.clone())]);
        assert!(has(drain(&mut first), route(URL, &list[0])));
        assert!(has(drain(&mut second), route(URL, &list[1])));
    }

    #[test]
    fn setting_a_relay_turns_a_live_route_off() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        app.apply(
            Action::SetRelay("https://relay.example.com".into()),
            &egui::Context::default(),
        );
        assert!(has(drain(&mut first), Command::PushPlugin(None)));
        assert!(has(drain(&mut second), Command::PushPlugin(None)));
        same(drain(&mut service), vec![told(vec![])]);
    }

    #[test]
    fn account_add_and_remove_resend_channels() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut service);
        drain(&mut first);
        drain(&mut second);
        let (backend, mut third) = Backend::recording();
        app.accounts.push(Account::new(backend));
        app.sync_push_route(false);
        let list = channels(&app);
        assert_eq!(list.len(), 3);
        same(drain(&mut service), vec![told(list.clone())]);
        // Existing accounts keep their route; the new one gets it on start.
        assert!(drain(&mut first).is_empty());
        app.start_account(2);
        let started = drain(&mut third);
        let relay = started
            .iter()
            .position(|c| matches!(c, Command::Relay(_)))
            .unwrap();
        let pushed = started
            .iter()
            .position(|c| format!("{c:?}") == format!("{:?}", route(URL, &list[2])))
            .unwrap();
        assert!(relay < pushed);
        app.retire_slot(1);
        let list = channels(&app);
        assert_eq!(list.len(), 2);
        same(drain(&mut service), vec![told(list)]);
    }

    #[test]
    fn receives_lists_push() {
        let (app, ..) = setup(true);
        assert_eq!(app.plugin_receives("pr"), ["push"]);
        let (app, ..) = setup(false);
        assert!(app.plugin_receives("pr").is_empty());
    }

    #[test]
    fn view_follows_the_plugin() {
        let (mut app, ..) = setup(true);
        let view = app.push_plugin_view().unwrap();
        assert_eq!(view.name, "pr");
        assert_eq!(view.state, PushPluginState::Waiting);
        assert!(!view.relay_set);
        event(&mut app, "pr", ready(URL));
        assert_eq!(
            app.push_plugin_view().unwrap().state,
            PushPluginState::Up {
                host: "a.trycloudflare.com".into()
            }
        );
        app.settings.plugins[0].enabled = false;
        assert!(app.push_plugin_view().is_none());
    }

    #[test]
    fn a_restarted_run_must_announce_again() {
        let (mut app, mut service, mut first, _second) = setup(true);
        event(&mut app, "pr", ready(URL));
        drain(&mut service);
        drain(&mut first);
        // A restart (new generation) invalidates the old address.
        app.plugins_started.clear();
        app.sync_plugins();
        same(drain(&mut first), vec![Command::PushPlugin(None)]);
        assert_eq!(
            app.push_plugin_view().unwrap().state,
            PushPluginState::Waiting
        );
    }

    #[test]
    fn the_demo_sends_nothing() {
        let (mut app, mut service, mut first, mut second) = setup(true);
        app.demo = true;
        event(&mut app, "pr", ready(URL));
        assert!(drain(&mut service).is_empty());
        assert!(drain(&mut first).is_empty() && drain(&mut second).is_empty());
    }
}
