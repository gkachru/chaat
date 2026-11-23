//! The background side: sign-in, fetches, sends, and the cache.
//!
//! The loop reads commands one at a time and decides; anything that waits
//! on the network or the disk runs as its own task, so a slow request never
//! holds up another. Tasks report back through `Event`s to the interface,
//! and through `Internal` commands to this loop when its own state changes
//! (a finished sign-in, a failed fetch to forget).

mod chats;
mod fetch;
mod messages;
mod people;
mod plugins;
mod presence;
mod push;
mod push_loop;
mod search;
mod session;
mod updates;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, mpsc, watch};

use super::{
    AccountCommand, AccountEvent, AuthStatus, Command, Event, Merge, ServiceCommand, ServiceEvent,
    Waker,
};
use crate::auth::session::{AccountRecord, Session};
use crate::auth::token::{self, TokenError};
use crate::auth::{self, AuthConfig, CHANNEL_READ_SCOPE, IdClaims, loopback::Loopback};
use crate::graph::api::Outgoing;
use crate::graph::files::SharedFile;
use crate::graph::{Graph, GraphError};
use crate::model::{Access, ConversationId, Message, SearchHit};
use crate::paths::AppDirs;
use crate::realtime::{self, Notice, PushStatus};
use crate::secrets::SecretStore;
use crate::store::Store;
use crate::sync::{Freshness, PushBudget, Resource, Trigger};
use fetch::fetch_resource;
use messages::{DraftQueue, forget_outbox, remember_link, save};
use people::{People, contacts_of};
use plugins::{PluginStart, SecretQueue, plugin_secret_key, update_hello};
use push_loop::{PUSH_ACCOUNT, PushSource, PushTarget, push_loop};

/// How many messages of a conversation the cache hands over at once.
const CACHED_MESSAGES: usize = 200;
/// Photos are fetched again after a week.
const PHOTO_MAX_AGE: i64 = 7 * 24 * 60 * 60 * 1000;
/// Presence of one person is not asked for again within this.
const PRESENCE_REUSE: Duration = Duration::from_secs(60);
/// How long typing must pause before a search goes to Graph. The local
/// cache answers at once.
const TYPING_PAUSE: Duration = Duration::from_millis(400);

/// Whether search number `mine` is still the latest after the typing pause.
async fn still_latest(latest: &std::sync::atomic::AtomicU64, mine: u64) -> bool {
    tokio::time::sleep(TYPING_PAUSE).await;
    latest.load(std::sync::atomic::Ordering::SeqCst) == mine
}

/// Numbers a new search: the latest one from now on.
fn next_search(latest: &std::sync::atomic::AtomicU64) -> u64 {
    latest.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

/// Sends events and wakes the window for each.
#[derive(Clone)]
struct Outbox {
    events: std::sync::mpsc::Sender<Event>,
    waker: Waker,
}

impl Outbox {
    fn send(&self, event: impl Into<Event>) {
        let _ = self.events.send(event.into());
        self.waker.wake();
    }

    fn error(&self, context: &str, error: &GraphError) {
        // Throttling and lost connections recover on their own; say so
        // once, briefly.
        let text = match error {
            GraphError::Network(reason) => format!("{context}: offline ({reason})"),
            GraphError::Throttled { .. } => {
                format!("{context}: Microsoft is limiting requests, try again shortly")
            }
            other => format!("{context}: {other}"),
        };
        log::warn!("{text}");
        self.send(AccountEvent::Error(text));
    }
}

/// A signed-in account's live pieces.
struct Account {
    record: AccountRecord,
    graph: Arc<Graph>,
    store: Arc<Mutex<Store>>,
}

enum Internal {
    SignedIn(Box<(Session, IdClaims)>),
    SignInFailed(String),
    FetchFailed(Resource),
    /// Confirms the person's own change with a date-ranged refresh.
    Refetch(ConversationId),
    /// Reads back a resource the person just changed.
    Confirm(Resource),
    /// Graph said a chat changed.
    Pushed(String),
    /// Graph lost notifications: refresh the chat list.
    PushMissed,
    /// Graph said someone's presence changed: the person's own (or unnamed),
    /// or a contact's.
    PresencePushed(Option<String>),
    /// Fetch the contacts whose presence changed since the last flush.
    FlushPresence,
    /// A chat list arrived: whose presence it shows (see `People`), and what
    /// caused the fetch.
    Contacts {
        people: People,
        trigger: Trigger,
    },
    /// A resend that found no copy of the message: post it.
    Post(crate::outbox::OutboxEntry),
}

/// A short tag for an account in log lines: the start of its object id. No
/// names or addresses.
fn account_tag(account: &str) -> String {
    account.chars().take(8).collect()
}

/// How long presence notices gather before one fetch for all of them.
const PRESENCE_BATCH: Duration = Duration::from_millis(1500);

struct Worker {
    dirs: AppDirs,
    /// The setup file opened and waiting to be applied.
    setup_slot: super::setup::SetupSlot,
    http: reqwest::Client,
    secrets: Arc<dyn SecretStore>,
    out: Outbox,
    internal: mpsc::UnboundedSender<Internal>,
    config: Option<AuthConfig>,
    account: Option<Account>,
    /// The account whose stored session did not restore, so signing in
    /// again can name it to Microsoft and keep its permissions.
    expired: Option<AccountRecord>,
    cancel_sign_in: Option<watch::Sender<bool>>,
    freshness: Freshness,
    /// How many per-chat message fetches pushes may still cause.
    push_budget: PushBudget,
    /// Conversations whose newest page was checked for a resent message
    /// since the account started; resends take turns.
    resend_checked: Arc<tokio::sync::Mutex<std::collections::HashSet<ConversationId>>>,
    /// The open account's cache, kept while its session is expired so drafts
    /// and unsent messages still save.
    store: Option<Arc<Mutex<Store>>>,
    /// Draft saves waiting to be written; the latest per conversation wins.
    drafts: Arc<DraftQueue>,
    presence_seen: HashMap<String, Instant>,
    /// Contacts whose presence Graph said changed, waiting for one fetch.
    presence_pushed: std::collections::HashSet<String>,
    /// The people of one-on-one chats, for the push loop's presence
    /// subscription.
    contacts: watch::Sender<Vec<String>>,
    photos_asked: HashMap<String, Instant>,
    names_asked: HashMap<String, Instant>,
    /// The latest message search and people search, by number: a search
    /// typed over by a newer one before `TYPING_PAUSE` never reaches Graph.
    searches: Arc<std::sync::atomic::AtomicU64>,
    people_searches: Arc<std::sync::atomic::AtomicU64>,
    /// Channel paging links, by conversation.
    older_links: Arc<Mutex<HashMap<ConversationId, String>>>,
    /// Whether the last fetch reached Microsoft (see `Event::Reachable`).
    reachable: Arc<std::sync::atomic::AtomicBool>,
    downloads: Arc<Semaphore>,
    options: super::BackendOptions,
    relay: Option<String>,
    /// Where a push plugin receives Graph's posts, used when no relay is set.
    push_route: Option<super::PushRoute>,
    /// Feeds bodies a push plugin forwarded to the running plugin loop.
    push_bodies: Option<mpsc::Sender<String>>,
    /// Whether this loop session already logged dropped bodies.
    push_dropped: bool,
    /// Stops the running push loop, and the loop itself, to wait for its
    /// cleanup.
    stop_push: Option<(watch::Sender<bool>, tokio::task::JoinHandle<()>)>,
    /// The plugins of the plugin list, by id.
    plugins: HashMap<String, crate::plugin::host::Plugin>,
    /// Plugins being stopped off the command loop, by task, with the id each
    /// one was running as.
    stopping: tokio::task::JoinSet<()>,
    stopping_ids: HashMap<tokio::task::Id, String>,
    /// The latest start asked for an id that is still stopping.
    deferred_starts: HashMap<String, PluginStart>,
    /// Credential store work for plugins, one queue per plugin id, created on
    /// first use. Within a plugin jobs run one at a time in the order given:
    /// a saved secret is seen by the options sent after it, and options reach
    /// a plugin in the order they were asked for. Plugins do not wait for
    /// each other.
    secret_jobs: HashMap<String, SecretQueue>,
    /// Whether Chaat keeps a presence session (setting), and when it last
    /// refreshed it.
    session_enabled: bool,
    session_refreshed: Option<Instant>,
    /// Where Microsoft's sign-in page opens (the call browser), if not the
    /// default browser.
    sign_in_browser: Option<crate::calling::browser::BrowserProfile>,
}

pub(super) async fn run(
    dirs: AppDirs,
    options: super::BackendOptions,
    events: std::sync::mpsc::Sender<Event>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    waker: Waker,
) {
    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();
    let http = reqwest::Client::builder()
        .user_agent(concat!("Chaat/", env!("CARGO_PKG_VERSION")))
        .gzip(true)
        .connect_timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .unwrap_or_default();
    let mut worker = Worker {
        dirs,
        setup_slot: Default::default(),
        http,
        secrets: Arc::clone(&options.secrets),
        options,
        relay: None,
        push_route: None,
        push_bodies: None,
        push_dropped: false,
        stop_push: None,
        plugins: HashMap::new(),
        stopping: tokio::task::JoinSet::new(),
        stopping_ids: HashMap::new(),
        deferred_starts: HashMap::new(),
        secret_jobs: HashMap::new(),
        session_enabled: true,
        session_refreshed: None,
        sign_in_browser: None,
        out: Outbox { events, waker },
        internal: internal_tx,
        config: None,
        account: None,
        expired: None,
        cancel_sign_in: None,
        freshness: Freshness::default(),
        push_budget: PushBudget::default(),
        resend_checked: Arc::default(),
        store: None,
        drafts: Arc::default(),
        presence_seen: HashMap::new(),
        presence_pushed: std::collections::HashSet::new(),
        contacts: watch::channel(Vec::new()).0,
        photos_asked: HashMap::new(),
        names_asked: HashMap::new(),
        searches: Arc::default(),
        people_searches: Arc::default(),
        older_links: Arc::default(),
        reachable: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        downloads: Arc::new(Semaphore::new(4)),
    };
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(Command::Shutdown) => break,
                Some(command) => worker.handle(command).await,
            },
            Some(internal) = internal_rx.recv() => worker.internal(internal).await,
            Some(stopped) = worker.stopping.join_next_with_id() => worker.plugin_stopped(stopped),
        }
    }
    if let Some(cancel) = worker.cancel_sign_in.take() {
        let _ = cancel.send(true);
    }
    worker.stop_push().await;
    worker.stop_plugins().await;
    worker.end_presence_session().await;
}

impl Worker {
    async fn handle(&mut self, command: Command) {
        match command {
            Command::Service(command) => self.handle_service(command).await,
            Command::Account(command) => self.handle_account(command).await,
            // The loop in `run` stops before a shutdown gets here.
            Command::Shutdown => {}
        }
    }

    async fn handle_service(&mut self, command: ServiceCommand) {
        match command {
            ServiceCommand::DiscardUpdate(prepared) => self.discard_update(prepared),
            ServiceCommand::AcknowledgeUpdate(receipt) => self.acknowledge_update(receipt),
            ServiceCommand::CheckUpdate { download } => self.check_update(download),
            ServiceCommand::DownloadUpdate(release) => self.download_release(release),
            ServiceCommand::InstallUpdate(prepared) => self.install_update(prepared),
            ServiceCommand::SaveRelayKey(key) => {
                let secrets = Arc::clone(&self.secrets);
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    let set = key.is_some();
                    let result = match key {
                        Some(key) => secrets.write(crate::secrets::RELAY_KEY, &key.0),
                        None => secrets.delete(crate::secrets::RELAY_KEY),
                    };
                    if result.is_err() {
                        log::warn!("could not save the relay access key");
                    }
                    out.send(ServiceEvent::RelayKeySaved {
                        set,
                        ok: result.is_ok(),
                    });
                });
            }
            ServiceCommand::StartPlugin {
                id,
                path,
                hello,
                expected_sha256,
                generation,
            } => self.start_plugin(
                id,
                PluginStart {
                    path,
                    hello,
                    expected_sha256,
                    generation,
                },
            ),
            ServiceCommand::PluginSend { id, message } => {
                if let Some(plugin) = self.plugins.get(&id) {
                    plugin.send(message);
                }
            }
            ServiceCommand::PluginHello {
                id,
                accounts,
                forget,
            } => {
                if let Some(plugin) = self.plugins.get(&id) {
                    plugin.set_hello(move |hello| update_hello(hello, accounts, forget));
                } else if let Some(start) = self.deferred_starts.get_mut(&id) {
                    update_hello(&mut start.hello, accounts, forget);
                }
            }
            ServiceCommand::PluginOptions {
                id,
                values,
                secrets,
            } => self.plugin_options(id, values, secrets),
            ServiceCommand::SavePluginSecret {
                plugin,
                instance,
                option,
                value,
            } => {
                let store = Arc::clone(&self.secrets);
                let out = self.out.clone();
                self.secret_queue(&plugin).run(move || {
                    let key = plugin_secret_key(&plugin, &option);
                    let set = value.is_some();
                    let result = match value {
                        Some(value) => store.write(&key, &value),
                        None => store.delete(&key),
                    };
                    if result.is_err() {
                        log::warn!("could not save a plugin secret");
                    }
                    out.send(ServiceEvent::PluginSecretSaved {
                        plugin,
                        instance,
                        option,
                        set,
                        ok: result.is_ok(),
                    });
                });
            }
            ServiceCommand::StopPlugin { id } => self.stop_plugin(id),
            ServiceCommand::OpenSetupFile {
                path,
                passphrase,
                token,
            } => {
                let slot = Arc::clone(&self.setup_slot);
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    let today = jiff::Zoned::now().date();
                    let result = super::setup::open(&path, &passphrase, token, today, &slot);
                    out.send(ServiceEvent::SetupOpened { token, result });
                });
            }
            ServiceCommand::ApplySetup { token } => {
                let slot = Arc::clone(&self.setup_slot);
                let store = Arc::clone(&self.secrets);
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    let ok = super::setup::apply(token, &slot, store.as_ref());
                    out.send(ServiceEvent::SetupApplied { token, ok });
                });
            }
            ServiceCommand::DiscardSetup { token } => {
                let slot = Arc::clone(&self.setup_slot);
                tokio::task::spawn_blocking(move || super::setup::discard(token, &slot));
            }
            ServiceCommand::ExportSetup {
                path,
                payload,
                include_relay_key,
            } => {
                let store = Arc::clone(&self.secrets);
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    let result = super::setup_export::export(
                        &path,
                        payload,
                        include_relay_key,
                        store.as_ref(),
                    );
                    out.send(ServiceEvent::SetupExported(result));
                });
            }
        }
    }

    async fn handle_account(&mut self, command: AccountCommand) {
        match command {
            AccountCommand::Start {
                client_id,
                tenant,
                account,
            } => self.start(client_id, tenant, account).await,
            AccountCommand::SignIn {
                extra_scopes,
                device_code,
            } => self.sign_in(extra_scopes, device_code),
            AccountCommand::CancelSignIn => {
                if let Some(cancel) = self.cancel_sign_in.take() {
                    let _ = cancel.send(true);
                }
            }
            AccountCommand::SignOut => self.sign_out().await,
            AccountCommand::Fetch { resource, trigger } => self.fetch(resource, trigger),
            AccountCommand::CachedMessages(conversation) => self.cached_messages(conversation),
            AccountCommand::AgentRead {
                plugin,
                request,
                query,
                access,
            } => self.agent_read(plugin, request, query, access),
            AccountCommand::LoadOlder {
                conversation,
                before,
            } => self.load_older(conversation, before),
            AccountCommand::LoadReplies { conversation, root } => {
                self.load_replies(conversation, root)
            }
            AccountCommand::Paste {
                conversation,
                range,
                text,
            } => {
                let out = self.out.clone();
                tokio::spawn(async move {
                    let image = tokio::task::spawn_blocking(crate::clipboard::read_image)
                        .await
                        .unwrap_or(Err("The clipboard could not be read."));
                    out.send(AccountEvent::Pasted {
                        conversation,
                        range,
                        text,
                        image,
                    });
                });
            }
            AccountCommand::Send(crate::outbox::SendPayload {
                conversation,
                local_id,
                html,
                reply_to,
                mentions,
                files,
                images,
                recipients,
            }) => self.send(
                conversation,
                local_id,
                Outgoing {
                    html,
                    mentions,
                    files: Vec::new(),
                    images,
                },
                reply_to,
                files,
                recipients,
            ),
            AccountCommand::SaveDraft {
                conversation,
                draft,
            } => {
                if let Some(store) = &self.store {
                    self.drafts.push(store, conversation, draft);
                }
            }
            AccountCommand::SaveOutbox(entry) => {
                if let Some(store) = &self.store {
                    let store = Arc::clone(store);
                    let _ = tokio::task::spawn_blocking(move || {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        if let Err(error) = store.save_outbox(&entry) {
                            log::warn!("could not save an unsent message: {error}");
                        }
                    })
                    .await;
                }
            }
            AccountCommand::DropOutbox(local_id) => {
                if let Some(store) = &self.store {
                    forget_outbox(store, &local_id).await;
                }
            }
            AccountCommand::Resend(entry) => self.resend(entry),
            AccountCommand::SearchIn {
                conversation,
                query,
                remote,
            } => self.search_in(conversation, query, remote),
            AccountCommand::SearchPeople(query) => self.search_people(query),
            AccountCommand::CreateChat { others, topic } => self.create_chat(others, topic),
            AccountCommand::AddMembers {
                chat,
                users,
                history,
            } => self.add_members(chat, users, history),
            AccountCommand::LeaveChat(chat) => self.leave_chat(chat),
            AccountCommand::SetAvailability(availability) => self.set_availability(availability),
            AccountCommand::SetStatusMessage { text, expires } => {
                self.set_status_message(text, expires)
            }
            AccountCommand::SetOutOfOffice(value) => self.set_out_of_office(value),
            AccountCommand::PresenceActive => self.presence_active(),
            AccountCommand::SignInBrowser(browser) => self.sign_in_browser = browser,
            AccountCommand::FindMeetingChat(join_url) => self.find_meeting_chat(join_url),
            AccountCommand::PresenceSession(enabled) => {
                self.session_enabled = enabled;
                if !enabled {
                    self.end_presence_session().await;
                }
            }
            AccountCommand::Edit {
                conversation,
                message,
                html,
            } => self.edit(conversation, message, html),
            AccountCommand::Delete {
                conversation,
                message,
            } => self.delete(conversation, message),
            AccountCommand::React {
                conversation,
                message,
                emoji,
                on,
            } => self.react(conversation, message, emoji, on),
            AccountCommand::MarkRead(chat) => self.mark_read(chat),
            AccountCommand::HideChat { chat, hide } => self.hide_chat(chat, hide),
            AccountCommand::MarkChatUnread(chat) => self.mark_chat_unread(chat),
            AccountCommand::PinMessage { chat, message } => self.pin_message(chat, message),
            AccountCommand::UnpinMessage { chat, pin } => self.unpin_message(chat, pin),
            AccountCommand::FindMeetingById(id) => self.find_meeting_by_id(id),
            AccountCommand::Relay(relay) => {
                self.relay = relay.as_deref().and_then(realtime::normalize_relay);
                if relay.is_some() && self.relay.is_none() {
                    self.out.send(AccountEvent::Push(PushStatus::Failed(
                        "the relay must be an https:// address".into(),
                    )));
                }
                self.restart_push().await;
            }
            AccountCommand::PushPlugin(route) => {
                self.push_route = route.and_then(|route| {
                    let url = realtime::normalize_relay(&route.url)?;
                    Some(super::PushRoute::new(url, route.channel))
                });
                // A relay set by hand wins: the route is kept for later, and
                // the running relay session is left alone.
                if self.relay.is_none() {
                    self.restart_push().await;
                }
            }
            AccountCommand::PushBody(body) => {
                // Dropped when no plugin loop is running.
                let full = self
                    .push_bodies
                    .as_ref()
                    .is_some_and(|bodies| bodies.try_send(body).is_err());
                if full && !self.push_dropped {
                    self.push_dropped = true;
                    log::debug!("push: plugin bodies dropped (queue full or closed)");
                }
            }
            AccountCommand::Presence(users) => self.presence(users),
            AccountCommand::Photo(user) => self.photo(user),
            AccountCommand::UserName(user) => self.user_name(user),
            AccountCommand::Media(url) => self.media(url),
            AccountCommand::Search(query) => self.search(query),
        }
    }

    async fn internal(&mut self, internal: Internal) {
        match internal {
            Internal::SignedIn(boxed) => {
                let (session, claims) = *boxed;
                self.cancel_sign_in = None;
                self.install(session, claims).await;
            }
            Internal::SignInFailed(reason) => {
                self.cancel_sign_in = None;
                self.out
                    .send(AccountEvent::Auth(AuthStatus::Failed(reason)));
            }
            Internal::FetchFailed(resource) => self.freshness.forget(&resource),
            Internal::Post(entry) => self.send(
                entry.payload.conversation,
                entry.payload.local_id,
                Outgoing {
                    html: entry.payload.html,
                    mentions: entry.payload.mentions,
                    files: Vec::new(),
                    images: entry.payload.images,
                },
                entry.payload.reply_to,
                entry.payload.files,
                entry.payload.recipients,
            ),
            Internal::Refetch(conversation) => {
                self.fetch(Resource::Messages(conversation), Trigger::AfterAction)
            }
            Internal::Confirm(resource) => self.fetch(resource, Trigger::AfterAction),
            Internal::Pushed(chat) => {
                // Over the budget the chat's messages wait until the person
                // looks; the list below still shows the change.
                // A notice for a chat fetched under a second ago costs
                // nothing: `fetch` would skip it anyway.
                let now = Instant::now();
                let resource = Resource::Messages(ConversationId::Chat(chat));
                if self.freshness.due(&resource, Trigger::Push, now) {
                    if self.push_budget.take(now) {
                        self.fetch(resource, Trigger::Push);
                    } else {
                        log::debug!("push: over the fetch budget; a chat waits");
                    }
                }
                self.fetch(Resource::Chats, Trigger::Push);
            }
            Internal::PushMissed => self.fetch(Resource::Chats, Trigger::Push),
            Internal::PresencePushed(user) => {
                let me = self.me_id();
                match user {
                    Some(user) if Some(&user) != me.as_ref() => {
                        // A burst of changes is one fetch, a moment later.
                        if self.presence_pushed.is_empty() {
                            let internal = self.internal.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(PRESENCE_BATCH).await;
                                let _ = internal.send(Internal::FlushPresence);
                            });
                        }
                        self.presence_pushed.insert(user);
                    }
                    _ => self.fetch(Resource::MyPresence, Trigger::Push),
                }
            }
            Internal::FlushPresence => {
                let users: Vec<String> = self.presence_pushed.drain().collect();
                self.presence_changed(users);
            }
            Internal::Contacts { people, trigger } => {
                // Looking at the chat list shows who is around. A list
                // fetched because of a push leaves presence to its own
                // notifications.
                if trigger != Trigger::Push {
                    self.presence(people.all);
                }
                self.contacts.send_replace(people.followable);
            }
        }
    }

    // -- Accounts ---------------------------------------------------------

    // -- Push ---------------------------------------------------------------

    // -- Fetching -----------------------------------------------------------

    // -- Writing ------------------------------------------------------------

    // -- People and presence --------------------------------------------------

    // -- People and media ---------------------------------------------------

    /// The signed-in person's user id.
    fn me_id(&self) -> Option<String> {
        self.account
            .as_ref()
            .map(|account| account.record.user_id().to_owned())
    }
}
