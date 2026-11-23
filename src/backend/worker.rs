//! The background side: sign-in, fetches, sends, and the cache.
//!
//! The loop reads commands one at a time and decides; anything that waits
//! on the network or the disk runs as its own task, so a slow request never
//! holds up another. Tasks report back through `Event`s to the interface,
//! and through `Internal` commands to this loop when its own state changes
//! (a finished sign-in, a failed fetch to forget).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, mpsc, watch};

use super::{AuthStatus, Command, Event, Merge, Waker};
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

/// How many messages of a conversation the cache hands over at once.
const CACHED_MESSAGES: usize = 200;
/// Photos are fetched again after a week.
const PHOTO_MAX_AGE: i64 = 7 * 24 * 60 * 60 * 1000;
/// Presence of one person is not asked for again within this.
const PRESENCE_REUSE: Duration = Duration::from_secs(60);

/// Sends events and wakes the window for each.
#[derive(Clone)]
struct Outbox {
    events: std::sync::mpsc::Sender<Event>,
    waker: Waker,
}

impl Outbox {
    fn send(&self, event: Event) {
        let _ = self.events.send(event);
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
        self.send(Event::Error(text));
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

tokio::task_local! {
    /// The account a push loop serves, for its log lines (see `push_log!`).
    static PUSH_ACCOUNT: String;
}

/// Logs a push line tagged with the account the current push loop serves,
/// so two accounts' lines can be told apart.
macro_rules! push_log {
    ($level:ident, $($arg:tt)+) => {
        log::$level!(
            "push[{}]: {}",
            PUSH_ACCOUNT.try_with(String::clone).unwrap_or_default(),
            format_args!($($arg)+)
        )
    };
}

/// A short tag for an account in log lines: the start of its object id. No
/// names or addresses.
fn account_tag(account: &str) -> String {
    account.chars().take(8).collect()
}

/// How long stopping push may wait for its subscriptions to be deleted.
const PUSH_CLEANUP: Duration = Duration::from_secs(3);

/// Bodies a push plugin may have waiting for the push loop; more are dropped.
const PUSH_BODY_QUEUE: usize = 64;

/// A subscription is reauthorized at most this often, however many
/// `reauthorizationRequired` notices arrive.
const REAUTHORIZE_GAP: Duration = Duration::from_secs(10 * 60);

/// A push session that stayed live this long was healthy: its end starts
/// the retry wait over.
const HEALTHY_PUSH_SESSION: Duration = Duration::from_secs(2 * 60);

/// How long presence notices gather before one fetch for all of them.
const PRESENCE_BATCH: Duration = Duration::from_millis(1500);

/// The credential store key of a plugin's secret option.
fn plugin_secret_key(plugin: &str, option: &str) -> String {
    format!("plugin-option:{plugin}:{option}")
}

/// One plugin's credential store queue: its jobs run one at a time, in the
/// order given, each on the blocking pool. A job that blocks (a locked or
/// prompting credential store) holds only this plugin's queue.
struct SecretQueue {
    jobs: mpsc::UnboundedSender<Box<dyn FnOnce() + Send>>,
    /// Jobs queued or running.
    pending: Arc<std::sync::atomic::AtomicUsize>,
}

/// Counts a job as done when dropped: after it ran, panicked or was dropped
/// unrun.
struct JobDone(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for JobDone {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl SecretQueue {
    /// Starts the task that runs the jobs. It ends when the queue is dropped
    /// and its jobs have run.
    fn start() -> Self {
        let (jobs, mut rx) = mpsc::unbounded_channel::<Box<dyn FnOnce() + Send>>();
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let _ = tokio::task::spawn_blocking(job).await;
            }
        });
        Self {
            jobs,
            pending: Arc::default(),
        }
    }

    fn run(&self, job: impl FnOnce() + Send + 'static) {
        self.pending
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let done = JobDone(Arc::clone(&self.pending));
        let _ = self.jobs.send(Box::new(move || {
            let _done = done;
            job();
        }));
    }

    fn idle(&self) -> bool {
        self.pending.load(std::sync::atomic::Ordering::SeqCst) == 0
    }
}

/// A plugin start held back until the previous run of the same id has
/// stopped, so two processes of one plugin never overlap.
struct PluginStart {
    path: std::path::PathBuf,
    hello: crate::plugin::protocol::HostMessage,
    expected_sha256: Option<String>,
    generation: u64,
}

/// Changes what a restart hello holds, for each part given.
fn update_hello(
    hello: &mut crate::plugin::protocol::HostMessage,
    accounts: Option<Vec<crate::plugin::protocol::PluginAccount>>,
    forget: Option<Vec<String>>,
) {
    if let crate::plugin::protocol::HostMessage::Hello {
        accounts: current_accounts,
        forget: current_forget,
        ..
    } = hello
    {
        if let Some(accounts) = accounts {
            *current_accounts = accounts;
        }
        if let Some(forget) = forget {
            *current_forget = forget;
        }
    }
}

/// How long shutdown waits for plugins to stop. Each stop is bounded on its
/// own (`Plugin::stop`); this only guards the whole.
const PLUGIN_STOP_WAIT: Duration = Duration::from_secs(10);

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
            Command::DiscardUpdate(prepared) => {
                tokio::task::spawn_blocking(move || prepared.discard());
            }
            Command::AcknowledgeUpdate(receipt) => {
                tokio::task::spawn_blocking(move || {
                    if receipt.acknowledge().is_err() {
                        log::warn!("could not acknowledge the app update");
                    }
                });
            }
            Command::CheckUpdate { download } => {
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    let result = crate::updates::updater().and_then(|updater| {
                        match updater.check()? {
                            None => out.send(Event::Update(crate::updates::Event::Status(
                                crate::updates::Status::Current,
                            ))),
                            Some(release) => {
                                let reason = updater.installation().err().map(|r| r.to_string());
                                if download && reason.is_none() {
                                    download_update(&out, &updater, &release)?;
                                } else {
                                    out.send(Event::Update(crate::updates::Event::Status(
                                        crate::updates::Status::Available { release, reason },
                                    )));
                                }
                            }
                        }
                        Ok(())
                    });
                    if result.is_err() {
                        out.send(Event::Update(crate::updates::Event::Status(crate::updates::Status::Failed("Could not check for updates. No public release may be available yet; try again later."))));
                    }
                });
            }
            Command::DownloadUpdate(release) => {
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    if crate::updates::updater()
                        .and_then(|updater| download_update(&out, &updater, &release))
                        .is_err()
                    {
                        out.send(Event::Update(crate::updates::Event::Status(crate::updates::Status::Failed("Could not download or verify the update. Your installed app has not changed."))));
                    }
                });
            }
            Command::InstallUpdate(prepared) => {
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    if crate::updates::updater()
                        .and_then(|updater| updater.handoff(*prepared, Vec::new()))
                        .is_ok()
                    {
                        out.send(Event::Update(crate::updates::Event::Quit));
                    } else {
                        out.send(Event::Update(crate::updates::Event::Status(
                            crate::updates::Status::Failed(
                                "Could not start the update. Your installed app has not changed.",
                            ),
                        )));
                    }
                });
            }
            Command::Start {
                client_id,
                tenant,
                account,
            } => self.start(client_id, tenant, account).await,
            Command::SignIn {
                extra_scopes,
                device_code,
            } => self.sign_in(extra_scopes, device_code),
            Command::CancelSignIn => {
                if let Some(cancel) = self.cancel_sign_in.take() {
                    let _ = cancel.send(true);
                }
            }
            Command::SignOut => self.sign_out().await,
            Command::Fetch { resource, trigger } => self.fetch(resource, trigger),
            Command::CachedMessages(conversation) => self.cached_messages(conversation),
            Command::AgentRead {
                plugin,
                request,
                query,
                access,
            } => self.agent_read(plugin, request, query, access),
            Command::LoadOlder {
                conversation,
                before,
            } => self.load_older(conversation, before),
            Command::LoadReplies { conversation, root } => self.load_replies(conversation, root),
            Command::Paste {
                conversation,
                range,
                text,
            } => {
                let out = self.out.clone();
                tokio::spawn(async move {
                    let image = tokio::task::spawn_blocking(crate::clipboard::read_image)
                        .await
                        .unwrap_or(Err("The clipboard could not be read."));
                    out.send(Event::Pasted {
                        conversation,
                        range,
                        text,
                        image,
                    });
                });
            }
            Command::Send {
                conversation,
                local_id,
                html,
                reply_to,
                mentions,
                files,
                images,
                recipients,
            } => self.send(
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
            Command::SaveDraft {
                conversation,
                draft,
            } => {
                if let Some(store) = &self.store {
                    self.drafts.push(store, conversation, draft);
                }
            }
            Command::SaveOutbox(entry) => {
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
            Command::DropOutbox(local_id) => {
                if let Some(store) = &self.store {
                    forget_outbox(store, &local_id).await;
                }
            }
            Command::Resend(entry) => self.resend(entry),
            Command::SearchIn {
                conversation,
                query,
                remote,
            } => self.search_in(conversation, query, remote),
            Command::SearchPeople(query) => self.search_people(query),
            Command::CreateChat { others, topic } => self.create_chat(others, topic),
            Command::AddMembers {
                chat,
                users,
                history,
            } => self.add_members(chat, users, history),
            Command::LeaveChat(chat) => self.leave_chat(chat),
            Command::SetAvailability(availability) => self.set_availability(availability),
            Command::SetStatusMessage { text, expires } => self.set_status_message(text, expires),
            Command::SetOutOfOffice(value) => self.set_out_of_office(value),
            Command::PresenceActive => self.presence_active(),
            Command::SignInBrowser(browser) => self.sign_in_browser = browser,
            Command::FindMeetingChat(join_url) => self.find_meeting_chat(join_url),
            Command::PresenceSession(enabled) => {
                self.session_enabled = enabled;
                if !enabled {
                    self.end_presence_session().await;
                }
            }
            Command::Edit {
                conversation,
                message,
                html,
            } => self.edit(conversation, message, html),
            Command::Delete {
                conversation,
                message,
            } => self.delete(conversation, message),
            Command::React {
                conversation,
                message,
                emoji,
                on,
            } => self.react(conversation, message, emoji, on),
            Command::MarkRead(chat) => self.mark_read(chat),
            Command::HideChat { chat, hide } => self.hide_chat(chat, hide),
            Command::MarkChatUnread(chat) => self.mark_chat_unread(chat),
            Command::PinMessage { chat, message } => self.pin_message(chat, message),
            Command::UnpinMessage { chat, pin } => self.unpin_message(chat, pin),
            Command::FindMeetingById(id) => self.find_meeting_by_id(id),
            Command::SaveRelayKey(key) => {
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
                    out.send(Event::RelayKeySaved {
                        set,
                        ok: result.is_ok(),
                    });
                });
            }
            Command::Relay(relay) => {
                self.relay = relay.as_deref().and_then(realtime::normalize_relay);
                if relay.is_some() && self.relay.is_none() {
                    self.out.send(Event::Push(PushStatus::Failed(
                        "the relay must be an https:// address".into(),
                    )));
                }
                self.restart_push().await;
            }
            Command::PushPlugin(route) => {
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
            Command::PushBody(body) => {
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
            Command::Presence(users) => self.presence(users),
            Command::Photo(user) => self.photo(user),
            Command::UserName(user) => self.user_name(user),
            Command::Media(url) => self.media(url),
            Command::Search(query) => self.search(query),
            Command::StartPlugin {
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
            Command::PluginSend { id, message } => {
                if let Some(plugin) = self.plugins.get(&id) {
                    plugin.send(message);
                }
            }
            Command::PluginHello {
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
            Command::PluginOptions {
                id,
                values,
                secrets,
            } => self.plugin_options(id, values, secrets),
            Command::SavePluginSecret {
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
                    out.send(Event::PluginSecretSaved {
                        plugin,
                        instance,
                        option,
                        set,
                        ok: result.is_ok(),
                    });
                });
            }
            Command::StopPlugin { id } => self.stop_plugin(id),
            Command::OpenSetupFile {
                path,
                passphrase,
                token,
            } => {
                let slot = Arc::clone(&self.setup_slot);
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    let today = jiff::Zoned::now().date();
                    let result = super::setup::open(&path, &passphrase, token, today, &slot);
                    out.send(Event::SetupOpened { token, result });
                });
            }
            Command::ApplySetup { token } => {
                let slot = Arc::clone(&self.setup_slot);
                let store = Arc::clone(&self.secrets);
                let out = self.out.clone();
                tokio::task::spawn_blocking(move || {
                    let ok = super::setup::apply(token, &slot, store.as_ref());
                    out.send(Event::SetupApplied { token, ok });
                });
            }
            Command::DiscardSetup { token } => {
                let slot = Arc::clone(&self.setup_slot);
                tokio::task::spawn_blocking(move || super::setup::discard(token, &slot));
            }
            Command::ExportSetup {
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
                    out.send(Event::SetupExported(result));
                });
            }
            Command::Shutdown => {}
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
                self.out.send(Event::Auth(AuthStatus::Failed(reason)));
            }
            Internal::FetchFailed(resource) => self.freshness.forget(&resource),
            Internal::Post(entry) => self.send(
                entry.conversation,
                entry.local_id,
                Outgoing {
                    html: entry.html,
                    mentions: entry.mentions,
                    files: Vec::new(),
                    images: entry.images,
                },
                entry.reply_to,
                entry.files,
                entry.recipients,
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

    async fn start(&mut self, client_id: String, tenant: String, account: Option<String>) {
        if !crate::settings::is_guid(client_id.trim()) {
            self.config = None;
            self.out.send(Event::Auth(AuthStatus::NotConfigured));
            return;
        }
        let config = AuthConfig {
            authority: self.options.authority.clone(),
            ..AuthConfig::new(&client_id, &tenant)
        };
        let changed = self.config.as_ref() != Some(&config);
        self.config = Some(config.clone());
        if !changed && let Some(account) = &self.account {
            // The window marks the account as restoring whenever it sends
            // Start (saving an unchanged registration does), so say again
            // that it is signed in.
            let record = &account.record;
            self.out.send(Event::Auth(AuthStatus::SignedIn {
                account: record.id.clone(),
                name: record.display_name.clone(),
                username: record.username.clone(),
            }));
            return;
        }
        self.account = None;
        self.store = None;
        self.expired = None;
        let Some(id) = account else {
            self.out.send(Event::Auth(AuthStatus::SignedOut));
            return;
        };
        let record_path = self.dirs.account(&id).join("account.json");
        let Some(record) = AccountRecord::load(&record_path) else {
            self.out.send(Event::Auth(AuthStatus::SignedOut));
            return;
        };
        self.out.send(Event::Auth(AuthStatus::Restoring));
        // The cache first, so the window has something to show at once.
        let store = match Store::open(&self.dirs.database(&id)) {
            Ok(store) => Arc::new(Mutex::new(store)),
            Err(error) => {
                log::warn!("could not open the cache: {error}");
                match Store::in_memory() {
                    Ok(store) => Arc::new(Mutex::new(store)),
                    Err(_) => return,
                }
            }
        };
        self.store = Some(Arc::clone(&store));
        emit_cache(&store, &self.out).await;
        self.resend_checked.lock().await.clear();
        let restored_from = Arc::clone(&store);
        match Session::restore(
            &record,
            config,
            self.http.clone(),
            Arc::clone(&self.secrets),
        )
        .await
        {
            Ok(Some(session)) => {
                let session = Arc::new(session);
                self.out.send(Event::Granted(session.granted()));
                self.out
                    .send(Event::ChannelAccess(channel_access(&session)));
                self.account = Some(Account {
                    graph: Arc::new(Graph::with_base(
                        self.http.clone(),
                        session,
                        &self.options.graph_base,
                    )),
                    record: record.clone(),
                    store,
                });
                self.out.send(Event::Auth(AuthStatus::SignedIn {
                    account: record.id.clone(),
                    name: record.display_name.clone(),
                    username: record.username.clone(),
                }));
                emit_restored(&restored_from, &self.out).await;
                self.restart_push().await;
            }
            Ok(None) => {
                self.out.send(Event::Auth(AuthStatus::Expired {
                    username: record.username.clone(),
                    reason: "Sign in again to get new messages.".into(),
                }));
                emit_restored(&restored_from, &self.out).await;
                self.expired = Some(record);
            }
            Err(error) => {
                self.out.send(Event::Auth(AuthStatus::Expired {
                    username: record.username.clone(),
                    reason: format!("Chaat could not read its saved session: {error}."),
                }));
                emit_restored(&restored_from, &self.out).await;
                self.expired = Some(record);
            }
        }
    }

    fn sign_in(&mut self, extra_scopes: Vec<String>, device_code: bool) {
        let Some(config) = self.config.clone() else {
            self.out.send(Event::Auth(AuthStatus::NotConfigured));
            return;
        };
        if let Some(previous) = self.cancel_sign_in.take() {
            let _ = previous.send(true);
        }
        let (cancel, cancelled) = watch::channel(false);
        self.cancel_sign_in = Some(cancel);
        // Only scopes asked for now need a consent screen.
        let consent = !extra_scopes.is_empty();
        // Keep the scopes already granted when asking for more, and when
        // signing an expired account in again.
        let granted = match (&self.account, &self.expired) {
            (Some(account), _) => account.graph.session().granted(),
            (None, Some(record)) => record.granted.clone(),
            (None, None) => String::new(),
        };
        let mut extra = extra_scopes;
        for scope in auth::OPTIONAL_SCOPES {
            if auth::has_scope(&granted, scope) && !extra.iter().any(|s| s == scope) {
                extra.push((*scope).to_owned());
            }
        }
        let hint = self
            .account
            .as_ref()
            .map(|a| &a.record)
            .or(self.expired.as_ref())
            .map(|r| r.username.clone())
            .filter(|u| !u.is_empty());
        let http = self.http.clone();
        let secrets = Arc::clone(&self.secrets);
        let out = self.out.clone();
        let internal = self.internal.clone();
        let open_browser = self.options.open_browser;
        let sign_in_browser = self.sign_in_browser.clone();
        let stopped = cancelled.clone();
        tokio::spawn(async move {
            let result = if device_code {
                device_code_sign_in(&http, &config, &extra, &out, cancelled).await
            } else {
                browser_sign_in(
                    &http,
                    &config,
                    Ask {
                        extra: &extra,
                        hint: hint.as_deref(),
                        consent,
                    },
                    &out,
                    cancelled,
                    open_browser.then_some(sign_in_browser),
                )
                .await
            };
            let outcome = match result {
                Ok(response) => Session::from_sign_in(config, http, secrets, response)
                    .await
                    .map(|pair| Internal::SignedIn(Box::new(pair)))
                    .unwrap_or_else(|error| Internal::SignInFailed(error.to_string())),
                Err(reason) => Internal::SignInFailed(reason),
            };
            // A cancelled or replaced attempt reports nothing: the interface
            // has already moved on.
            if *stopped.borrow() {
                return;
            }
            let _ = internal.send(outcome);
        });
    }

    async fn install(&mut self, session: Session, claims: IdClaims) {
        let id = session.account.clone();
        let account_dir = self.dirs.account(&id);
        let _ = crate::paths::create_private_dir(&account_dir);
        let record = AccountRecord {
            id: id.clone(),
            display_name: claims.name.clone().unwrap_or_default(),
            username: claims.preferred_username.clone().unwrap_or_default(),
            tenant_id: claims.tid.clone().unwrap_or_default(),
            granted: session.granted(),
            client_id: session.config.client_id.clone(),
            tenant: session.config.tenant.clone(),
        };
        if let Err(error) = record.save(&account_dir.join("account.json")) {
            log::warn!("could not save the account record: {error}");
        }
        let store = match self.account.take() {
            // Re-consent for more scopes keeps the same cache.
            Some(previous) if previous.record.id == id => previous.store,
            _ => match Store::open(&self.dirs.database(&id)).or_else(|_| Store::in_memory()) {
                Ok(store) => Arc::new(Mutex::new(store)),
                Err(error) => {
                    self.out.send(Event::Auth(AuthStatus::Failed(format!(
                        "Could not open the cache: {error}"
                    ))));
                    return;
                }
            },
        };
        let saved = session.token_saved();
        let store_locked = session.store_locked();
        let session = Arc::new(session);
        self.out.send(Event::Granted(session.granted()));
        self.out
            .send(Event::ChannelAccess(channel_access(&session)));
        self.account = Some(Account {
            graph: Arc::new(Graph::with_base(
                self.http.clone(),
                session,
                &self.options.graph_base,
            )),
            record: record.clone(),
            store: Arc::clone(&store),
        });
        self.store = Some(Arc::clone(&store));
        self.expired = None;
        self.freshness.clear();
        emit_cache(&store, &self.out).await;
        self.out.send(Event::Auth(AuthStatus::SignedIn {
            account: id,
            name: record.display_name,
            username: record.username,
        }));
        if !saved {
            self.out
                .send(Event::Error(unsaved_sign_in_text(store_locked).into()));
        }
        self.restart_push().await;
    }

    // -- Push ---------------------------------------------------------------

    /// Stops any running push loop and, when a relay is set and an account is
    /// signed in, starts a new one.
    async fn restart_push(&mut self) {
        self.stop_push().await;
        let Some(account) = &self.account else {
            self.out.send(Event::Push(PushStatus::Off));
            return;
        };
        // A relay set by hand wins over a push plugin's route.
        let (mut source, bodies) = if let Some(relay) = self.relay.clone() {
            (PushSource::Relay { relay, key: None }, None)
        } else if let Some(route) = self.push_route.clone() {
            let (tx, rx) = mpsc::channel(PUSH_BODY_QUEUE);
            (PushSource::Plugin { route, bodies: rx }, Some(tx))
        } else {
            self.out.send(Event::Push(PushStatus::Off));
            return;
        };
        let (stop, stopped) = watch::channel(false);
        let graph = Arc::clone(&account.graph);
        let me = account.record.user_id().to_owned();
        let out = self.out.clone();
        let internal = self.internal.clone();
        let secrets = Arc::clone(&self.secrets);
        if let PushSource::Relay { key, .. } = &mut source {
            *key = tokio::task::spawn_blocking(move || secrets.read(crate::secrets::RELAY_KEY))
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
        }
        let target = PushTarget {
            me,
            source,
            leftovers: self
                .dirs
                .account(&account.record.id)
                .join(realtime::LEFTOVERS_FILE),
            contacts: self.contacts.subscribe(),
            refused: self
                .dirs
                .account(&account.record.id)
                .join(realtime::REFUSED_FILE),
            renew_every: self.options.push_renew_every,
            connect_within: self.options.relay_connect_timeout,
        };
        let tag = account_tag(&account.record.id);
        let task =
            tokio::spawn(PUSH_ACCOUNT.scope(tag, push_loop(graph, target, out, internal, stopped)));
        self.stop_push = Some((stop, task));
        self.push_bodies = bodies;
        self.push_dropped = false;
    }

    /// Stops the push loop and waits for it to delete its subscriptions,
    /// so they don't keep sending notifications to a closed channel (and a
    /// new presence subscription isn't refused as a duplicate). A slow or
    /// unreachable Graph cannot hold up quitting for longer than
    /// [`PUSH_CLEANUP`]; whatever is left is deleted at the next start.
    async fn stop_push(&mut self) {
        if let Some((stop, task)) = self.stop_push.take() {
            // Stop first: closing the body queue before the stop flag is set
            // would let the loop report "the push plugin stopped" as a failure.
            let _ = stop.send(true);
            self.push_bodies = None;
            if tokio::time::timeout(PUSH_CLEANUP, task).await.is_err() {
                log::info!(
                    "push[{}]: cleanup took too long; the next start finishes it",
                    self.me_id().as_deref().map(account_tag).unwrap_or_default()
                );
            }
        }
    }

    /// Launches (or relaunches) one plugin of the plugin list. Only that id's
    /// host is replaced. An id whose previous run is still stopping starts
    /// when that run has ended.
    fn start_plugin(&mut self, id: String, start: PluginStart) {
        if self.stopping_ids.values().any(|stopping| *stopping == id) {
            self.deferred_starts.insert(id, start);
            return;
        }
        self.launch_plugin(id, start);
    }

    fn launch_plugin(&mut self, id: String, start: PluginStart) {
        let out = self.out.clone();
        let tag = id.clone();
        let generation = start.generation;
        let plugin = crate::plugin::host::Plugin::start(
            start.path,
            start.hello,
            crate::plugin::registry::KNOWN,
            start.expected_sha256,
            move |reported| {
                if let Some(event) = crate::plugin::event::event(reported) {
                    out.send(Event::Plugin {
                        id: tag.clone(),
                        generation,
                        event,
                    });
                }
            },
        );
        if let Some(old) = self.plugins.insert(id, plugin) {
            old.abort();
        }
    }

    /// Stops one plugin without holding up the command loop: the stop runs in
    /// a tracked task, which shutdown waits for.
    fn stop_plugin(&mut self, id: String) {
        self.deferred_starts.remove(&id);
        if let Some(plugin) = self.plugins.remove(&id) {
            let task = self.stopping.spawn(plugin.stop());
            self.stopping_ids.insert(task.id(), id);
        }
        self.prune_secret_queues();
    }

    /// A stop task ended: starts what was waiting for it.
    fn plugin_stopped(&mut self, stopped: Result<(tokio::task::Id, ()), tokio::task::JoinError>) {
        let task = match stopped {
            Ok((task, ())) => task,
            Err(error) => {
                log::warn!("plugin: a stop task failed");
                error.id()
            }
        };
        let Some(id) = self.stopping_ids.remove(&task) else {
            return;
        };
        if !self.stopping_ids.values().any(|stopping| *stopping == id)
            && let Some(start) = self.deferred_starts.remove(&id)
        {
            self.launch_plugin(id, start);
        }
        self.prune_secret_queues();
    }

    /// One plugin's credential store queue, started on first use.
    fn secret_queue(&mut self, plugin: &str) -> &SecretQueue {
        self.prune_secret_queues();
        self.secret_jobs
            .entry(plugin.to_owned())
            .or_insert_with(SecretQueue::start)
    }

    /// Drops the queues of plugins that are not running and have nothing
    /// queued. A queue with work left stays, so a later job for the same
    /// plugin still runs after it.
    fn prune_secret_queues(&mut self) {
        let plugins = &self.plugins;
        self.secret_jobs
            .retain(|id, queue| !queue.idle() || plugins.contains_key(id));
    }

    /// Sends a plugin its options, with the stored secret values merged in.
    /// Values are never logged.
    fn plugin_options(
        &mut self,
        id: String,
        mut values: std::collections::BTreeMap<String, serde_json::Value>,
        secrets: Vec<String>,
    ) {
        let Some(plugin) = self.plugins.get(&id) else {
            return;
        };
        let store = Arc::clone(&self.secrets);
        let outgoing = plugin.sender();
        self.secret_queue(&id).run(move || {
            for option in secrets {
                if let Ok(Some(value)) = store.read(&plugin_secret_key(&id, &option)) {
                    values.insert(option, serde_json::Value::String(value));
                }
            }
            let _ = outgoing.send(crate::plugin::protocol::HostMessage::Options { values });
        });
    }

    /// Stops every plugin of the plugin list, all at once, and waits for
    /// every stop, including ones already under way.
    async fn stop_plugins(&mut self) {
        self.deferred_starts.clear();
        for (id, plugin) in self.plugins.drain() {
            let task = self.stopping.spawn(plugin.stop());
            self.stopping_ids.insert(task.id(), id);
        }
        let stops = &mut self.stopping;
        let all = async { while stops.join_next().await.is_some() {} };
        if tokio::time::timeout(PLUGIN_STOP_WAIT, all).await.is_err() {
            log::warn!("plugin: stopping took too long; giving up on the rest");
            self.stopping.abort_all();
        }
        self.stopping_ids.clear();
    }

    async fn sign_out(&mut self) {
        self.stop_push().await;
        self.end_presence_session().await;
        self.store = None;
        let Some(account) = self.account.take() else {
            self.out.send(Event::Auth(AuthStatus::SignedOut));
            return;
        };
        account.graph.session().sign_out().await;
        // The cache holds the person's messages: remove it with the session.
        // Fetches still running hold the database open, and Windows will
        // not delete an open file, so wait for them to let go first.
        let store = account.store;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&store) > 1 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(store);
        let dir = self.dirs.account(&account.record.id);
        for attempt in 1..=10 {
            match tokio::fs::remove_dir_all(&dir).await {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) if attempt == 10 => {
                    log::warn!("could not remove the account's cache: {error}");
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        self.freshness.clear();
        self.out.send(Event::Auth(AuthStatus::SignedOut));
    }

    // -- Fetching -----------------------------------------------------------

    fn fetch(&mut self, resource: Resource, trigger: Trigger) {
        let Some(account) = &self.account else { return };
        if !self
            .freshness
            .claim(resource.clone(), trigger, Instant::now())
        {
            if trigger == Trigger::Push {
                log::debug!(
                    "push[{}]: {} skipped, fetched under a second ago",
                    account_tag(&account.record.id),
                    describe(&resource)
                );
            }
            return;
        }
        if trigger == Trigger::Push {
            log::debug!(
                "push[{}]: fetching {}",
                account_tag(&account.record.id),
                describe(&resource)
            );
        }
        let tag = account_tag(&account.record.id);
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let internal = self.internal.clone();
        let record = account.record.clone();
        let links = Arc::clone(&self.older_links);
        let reachable = Arc::clone(&self.reachable);
        tokio::spawn(async move {
            use std::sync::atomic::Ordering;
            out.send(Event::Loading(resource.clone(), true));
            let result = fetch_resource(&graph, &store, &out, &resource, &record, &links).await;
            if trigger == Trigger::Push {
                log::debug!(
                    "push[{tag}]: {} done (ok: {})",
                    describe(&resource),
                    result.is_ok()
                );
            }
            out.send(Event::Loading(resource.clone(), false));
            let lost = result.as_ref().err().is_some_and(is_unreachable);
            if lost == reachable.swap(!lost, Ordering::Relaxed) {
                out.send(Event::Reachable(!lost));
            }
            if result.is_ok() && resource == Resource::Chats {
                let me = record.user_id().to_owned();
                let people = contacts_of(&store, me, record.tenant_id.clone()).await;
                let _ = internal.send(Internal::Contacts { people, trigger });
            }
            if lost {
                // Brief outages fix themselves on the next look: the app
                // shows a quiet marker instead of an error.
                log::warn!("{}: Microsoft unreachable", describe(&resource));
                let _ = internal.send(Internal::FetchFailed(resource.clone()));
            } else if let Err(error) = result {
                let _ = internal.send(Internal::FetchFailed(resource.clone()));
                match (&resource, &error) {
                    (
                        Resource::Messages(ConversationId::Channel { .. }),
                        GraphError::Forbidden { .. },
                    ) => {
                        out.send(Event::ChannelAccess(Access::Forbidden));
                    }
                    (Resource::MyPresence | Resource::Calendar, GraphError::Forbidden { .. }) => {
                        log::info!("{resource:?} is not permitted for this account");
                    }
                    _ => out.error(describe(&resource), &error),
                }
                if let GraphError::Auth(TokenError::Rejected(reason)) = &error {
                    out.send(Event::Auth(AuthStatus::Expired {
                        username: String::new(),
                        reason: reason.clone(),
                    }));
                }
            }
        });
    }

    fn load_older(&mut self, conversation: ConversationId, before: Option<i64>) {
        let Some(account) = &self.account else { return };
        if !self.freshness.claim(
            Resource::Messages(conversation.clone()),
            Trigger::History,
            Instant::now(),
        ) {
            return;
        }
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let links = Arc::clone(&self.older_links);
        tokio::spawn(async move {
            let resource = Resource::Messages(conversation.clone());
            out.send(Event::Loading(resource.clone(), true));
            let result = async {
                let page = match &conversation {
                    ConversationId::Chat(chat) => {
                        let cached = {
                            let store = store.lock().unwrap_or_else(|p| p.into_inner());
                            store
                                .messages_before(
                                    &conversation,
                                    before.unwrap_or(i64::MAX),
                                    CACHED_MESSAGES,
                                )
                                .unwrap_or_default()
                        };
                        if !cached.is_empty() {
                            remember_link(&links, &conversation, None);
                            out.send(Event::Messages {
                                conversation: conversation.clone(),
                                messages: cached,
                                merge: Merge::Older,
                                more: Some(true),
                                cached: true,
                            });
                            return Ok(None);
                        }
                        let link = links
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .get(&conversation)
                            .cloned();
                        let page = match link {
                            Some(link) => graph.chat_message_page(chat, &link).await?,
                            None => graph.chat_messages(chat, before).await?,
                        };
                        remember_link(&links, &conversation, page.older.clone());
                        page
                    }
                    ConversationId::Channel { team, channel } => {
                        let link = links
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .get(&conversation)
                            .cloned();
                        let Some(link) = link else {
                            return Ok(Some(false));
                        };
                        let page = graph.channel_messages(team, channel, Some(&link)).await?;
                        remember_link(&links, &conversation, page.older.clone());
                        page
                    }
                };
                let more = page.older.is_some() || page.messages.len() >= crate::graph::api::PAGE;
                save(&store, &conversation, &page.messages).await;
                out.send(Event::Messages {
                    conversation: conversation.clone(),
                    messages: page.messages,
                    merge: Merge::Older,
                    more: Some(more),
                    cached: false,
                });
                Ok::<_, GraphError>(None)
            }
            .await;
            out.send(Event::Loading(resource, false));
            match result {
                Ok(Some(more)) => out.send(Event::Messages {
                    conversation,
                    messages: Vec::new(),
                    merge: Merge::Older,
                    more: Some(more),
                    cached: false,
                }),
                Ok(None) => {}
                Err(error) => out.error("Loading older messages", &error),
            }
        });
    }

    fn load_replies(&mut self, conversation: ConversationId, root: String) {
        let Some(account) = &self.account else { return };
        let ConversationId::Channel { team, channel } = &conversation else {
            return;
        };
        let (team, channel) = (team.clone(), channel.clone());
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.channel_replies(&team, &channel, &root).await {
                Ok(replies) => out.send(Event::Replies {
                    conversation,
                    root,
                    replies,
                }),
                Err(error) => out.error("Loading replies", &error),
            }
        });
    }

    // -- Writing ------------------------------------------------------------

    fn send(
        &mut self,
        conversation: ConversationId,
        local_id: String,
        mut message: Outgoing,
        reply_to: Option<String>,
        files: Vec<std::path::PathBuf>,
        recipients: Vec<String>,
    ) {
        let Some(account) = &self.account else {
            self.out.send(Event::SendFailed {
                conversation,
                local_id,
                reason: "Not signed in".into(),
            });
            return;
        };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        tokio::spawn(async move {
            // Files first: upload, share with the conversation, reference.
            for path in &files {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match share_file(&graph, &conversation, path, &recipients, &out).await {
                    Ok(shared) => message.files.push(shared),
                    Err(error) => {
                        log::warn!("upload failed: {error}");
                        out.send(Event::SendFailed {
                            conversation,
                            local_id,
                            reason: format!("uploading {name} failed ({error})"),
                        });
                        return;
                    }
                }
            }
            let result = match &conversation {
                ConversationId::Chat(chat) => match &reply_to {
                    Some(quoted) => graph.reply_with_quote(chat, quoted, &message).await,
                    None => graph.send_chat_message(chat, &message).await,
                },
                ConversationId::Channel { team, channel } => {
                    graph
                        .send_channel_message(team, channel, reply_to.as_deref(), &message)
                        .await
                }
            };
            match result {
                Ok(message) => {
                    if message.reply_to.is_none() {
                        save(&store, &conversation, std::slice::from_ref(&message)).await;
                    }
                    forget_outbox(&store, &local_id).await;
                    out.send(Event::Sent {
                        conversation,
                        local_id,
                        message: Box::new(message),
                    });
                }
                Err(error) => {
                    log::warn!("send failed: {error}");
                    out.send(Event::SendFailed {
                        conversation,
                        local_id,
                        reason: error.to_string(),
                    });
                }
            }
        });
    }

    /// Sends a kept message again, unless the conversation already shows it.
    fn resend(&mut self, entry: crate::outbox::OutboxEntry) {
        let Some(account) = &self.account else {
            self.out.send(Event::SendFailed {
                conversation: entry.conversation,
                local_id: entry.local_id,
                reason: "Not signed in".into(),
            });
            return;
        };
        let resource = Resource::Messages(entry.conversation.clone());
        let claimed = self
            .freshness
            .claim(resource.clone(), Trigger::Start, Instant::now());
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let internal = self.internal.clone();
        let record = account.record.clone();
        let links = Arc::clone(&self.older_links);
        let checked = Arc::clone(&self.resend_checked);
        tokio::spawn(async move {
            let mut checked = checked.lock().await;
            if !checked.contains(&entry.conversation) {
                // Without a fresh look there is no telling whether Graph
                // took the message, so it is not sent blind.
                let fetched = claimed
                    && fetch_resource(&graph, &store, &out, &resource, &record, &links)
                        .await
                        .is_ok();
                if !fetched {
                    if claimed {
                        let _ = internal.send(Internal::FetchFailed(resource));
                    }
                    out.send(Event::SendFailed {
                        conversation: entry.conversation,
                        local_id: entry.local_id,
                        reason: "Chaat could not check whether it was sent. Retry to send it."
                            .into(),
                    });
                    return;
                }
                checked.insert(entry.conversation.clone());
            }
            drop(checked);
            let me = record.user_id().to_owned();
            let recent = {
                let key = entry.conversation.clone();
                let store = Arc::clone(&store);
                tokio::task::spawn_blocking(move || {
                    let store = store.lock().unwrap_or_else(|p| p.into_inner());
                    store.messages(&key, CACHED_MESSAGES).unwrap_or_default()
                })
                .await
                .unwrap_or_default()
            };
            match crate::outbox::already_sent(&entry, &recent, &me) {
                Some(message) => {
                    forget_outbox(&store, &entry.local_id).await;
                    out.send(Event::Sent {
                        conversation: entry.conversation,
                        local_id: entry.local_id,
                        message: Box::new(message),
                    });
                }
                None => {
                    let _ = internal.send(Internal::Post(entry));
                }
            }
        });
    }

    // -- People and presence --------------------------------------------------

    fn search_people(&mut self, query: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.search_people(&query).await {
                Ok(people) => out.send(Event::People { query, people }),
                Err(error) => log::info!("people search unavailable: {error}"),
            }
        });
    }

    fn create_chat(&mut self, others: Vec<String>, topic: Option<String>) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            match graph.create_chat(&me, &others, topic.as_deref()).await {
                Ok(chat) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = store.upsert_chat(&chat);
                    }
                    out.send(Event::ChatCreated(Box::new(chat)));
                }
                Err(error) => out.error("Starting the chat", &error),
            }
        });
    }

    fn add_members(&mut self, chat: String, users: Vec<String>, history: bool) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            let mut added = Vec::new();
            let mut failed = Vec::new();
            for user in users {
                match graph.add_chat_member(&chat, &user, history).await {
                    Ok(()) => added.push(user),
                    Err(error) => {
                        log::info!("adding a person to a chat failed: {error}");
                        failed.push(user);
                    }
                }
            }
            out.send(Event::MembersAdded {
                chat: chat.clone(),
                added,
                failed,
            });
            // Read the list back, so what is shown is what Graph has.
            let _ = internal.send(Internal::Confirm(Resource::Members(chat)));
        });
    }

    fn leave_chat(&mut self, chat: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            // The membership id is read now, never kept: it changes when
            // the person is removed and added again.
            let result = async {
                let mine = graph
                    .chat_members(&chat)
                    .await?
                    .into_iter()
                    .find(|(_, member)| member.user_id == me)
                    .map(|(membership, _)| membership);
                match mine {
                    Some(membership) => graph.remove_chat_member(&chat, &membership).await,
                    None => Err(GraphError::Decode),
                }
            }
            .await;
            match result {
                Ok(()) => {
                    {
                        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                        if let Ok(mut chats) = store.chats() {
                            chats.retain(|c| c.id != chat);
                            let _ = store.save_chats(&chats);
                        }
                    }
                    out.send(Event::LeftChat(chat));
                }
                Err(error) => {
                    log::info!("leaving a chat failed: {error}");
                    out.send(Event::LeaveFailed {
                        chat,
                        admin: matches!(error, GraphError::Forbidden { .. }),
                    });
                }
            }
        });
    }

    fn find_meeting_chat(&mut self, join_url: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        tokio::spawn(async move {
            let chat = match graph.meeting_chat(&join_url).await {
                Ok(Some(chat)) => {
                    let store = store.lock().unwrap_or_else(|p| p.into_inner());
                    let _ = store.upsert_chat(&chat);
                    Some(Box::new(chat))
                }
                Ok(None) => None,
                Err(error) => {
                    log::info!("meeting chat lookup failed: {error}");
                    None
                }
            };
            out.send(Event::MeetingChat { join_url, chat });
        });
    }

    fn set_availability(&mut self, availability: Option<crate::model::Availability>) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            match graph.set_preferred_presence(&me, availability).await {
                Ok(()) => {
                    if let Ok(presence) = graph.my_presence().await {
                        out.send(Event::MyPresence(presence));
                    }
                }
                Err(error) => out.error("Setting your status", &error),
            }
        });
    }

    fn set_status_message(&mut self, text: String, expires: Option<crate::model::Millis>) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            match graph.set_status_message(&me, &text, expires).await {
                Ok(()) => {
                    if let Ok(presence) = graph.my_presence().await {
                        out.send(Event::MyPresence(presence));
                    }
                }
                Err(error) => out.error("Setting your status message", &error),
            }
        });
    }

    /// Saves the automatic replies, shows them as saved, and reads them back.
    fn set_out_of_office(&mut self, value: crate::model::OutOfOffice) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            match graph.set_out_of_office(&value).await {
                Ok(()) => {
                    out.send(Event::OutOfOffice(value));
                    let _ = internal.send(Internal::Confirm(Resource::OutOfOffice));
                }
                Err(error) => {
                    if crate::graph::mailbox::no_mailbox(&error) {
                        out.send(Event::OutOfOfficeUnavailable);
                    } else {
                        out.error("Saving your automatic replies", &error);
                    }
                    out.send(Event::OutOfOfficeNotSaved);
                }
            }
        });
    }

    /// The person is using Chaat. An Available presence session turns
    /// inactive five minutes after its last refresh and Away five minutes
    /// later, so Chaat refreshes it only while someone is at the keyboard;
    /// stepping away lets Graph mark them Away, as a Teams client would.
    fn presence_active(&mut self) {
        let Some(account) = &self.account else { return };
        let (Some(config), true) = (&self.config, self.session_enabled) else {
            return;
        };
        if !account
            .graph
            .session()
            .has_scope(auth::PRESENCE_WRITE_SCOPE)
        {
            return;
        }
        let now = Instant::now();
        if self
            .session_refreshed
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(4 * 60))
        {
            return;
        }
        self.session_refreshed = Some(now);
        let graph = Arc::clone(&account.graph);
        let me = account.record.user_id().to_owned();
        let client = config.client_id.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.set_presence_session(&me, &client, false, 60).await {
                log::info!("could not refresh the presence session: {error}");
            }
        });
    }

    /// Ends Chaat's presence session, if it started one (at sign-out, when
    /// turned off, and at exit).
    async fn end_presence_session(&mut self) {
        if self.session_refreshed.take().is_none() {
            return;
        }
        let (Some(account), Some(config)) = (&self.account, &self.config) else {
            return;
        };
        let me = account.record.user_id().to_owned();
        let clear = account.graph.clear_presence_session(&me, &config.client_id);
        if let Ok(Err(error)) = tokio::time::timeout(Duration::from_secs(3), clear).await {
            log::info!("could not end the presence session: {error}");
        }
    }

    fn edit(&mut self, conversation: ConversationId, message: String, html: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            match graph.edit_message(&conversation, &message, &html).await {
                Ok(()) => {
                    let _ = internal.send(Internal::Refetch(conversation));
                }
                Err(error) => out.error("Editing the message", &error),
            }
        });
    }

    fn delete(&mut self, conversation: ConversationId, message: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            match graph.delete_message(&me, &conversation, &message).await {
                Ok(()) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = store.delete_message(&conversation, &message);
                    }
                    out.send(Event::Deleted {
                        conversation,
                        message,
                    });
                }
                Err(error) => out.error("Deleting the message", &error),
            }
        });
    }

    fn react(&mut self, conversation: ConversationId, message: String, emoji: String, on: bool) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            match graph.react(&conversation, &message, &emoji, on).await {
                Ok(()) => {
                    let _ = internal.send(Internal::Refetch(conversation));
                }
                Err(error) => out.error("Reacting", &error),
            }
        });
    }

    fn mark_read(&mut self, chat: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let me = account.record.user_id().to_owned();
        let tenant = account.record.tenant_id.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.mark_chat_read(&chat, &me, &tenant).await {
                log::info!("could not mark a chat read: {error}");
            }
        });
    }

    fn hide_chat(&mut self, chat: String, hide: bool) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        let tenant = account.record.tenant_id.clone();
        tokio::spawn(async move {
            match graph.hide_chat(&chat, &me, &tenant, hide).await {
                Ok(()) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(mut cached) = store
                            .chats()
                            .ok()
                            .and_then(|chats| chats.into_iter().find(|c| c.id == chat))
                        {
                            cached.hidden = hide;
                            let _ = store.upsert_chat(&cached);
                        }
                    }
                    out.send(Event::ChatHidden {
                        chat,
                        hidden: hide,
                        ok: true,
                    });
                }
                Err(error) => {
                    log::info!("hiding a chat failed: {error}");
                    out.send(Event::ChatHidden {
                        chat,
                        hidden: hide,
                        ok: false,
                    });
                }
            }
        });
    }

    fn mark_chat_unread(&mut self, chat: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        let tenant = account.record.tenant_id.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.mark_chat_unread(&chat, &me, &tenant).await {
                log::info!("could not mark a chat unread: {error}");
                out.send(Event::MarkUnreadFailed(chat));
            }
        });
    }

    fn pin_message(&mut self, chat: String, message: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.pin_message(&chat, &message).await {
                out.error("Pinning", &error);
            }
            // Success or not, read the pins back so the banner is true.
            let _ = internal.send(Internal::Confirm(Resource::Pins(chat)));
        });
    }

    fn unpin_message(&mut self, chat: String, pin: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.unpin_message(&chat, &pin).await {
                out.error("Unpinning", &error);
            }
            let _ = internal.send(Internal::Confirm(Resource::Pins(chat)));
        });
    }

    fn find_meeting_by_id(&mut self, id: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            let join_url = match graph.meeting_join_url_by_id(&id).await {
                Ok(url) => url,
                Err(error) => {
                    log::info!("meeting ID lookup failed: {error}");
                    None
                }
            };
            out.send(Event::MeetingById { id, join_url });
        });
    }

    // -- People and media ---------------------------------------------------

    fn presence(&mut self, users: Vec<String>) {
        let Some(account) = &self.account else { return };
        if !account.graph.session().has_scope("Presence.Read.All") {
            return;
        }
        let now = Instant::now();
        let due: Vec<String> = users
            .into_iter()
            .filter(|user| {
                self.presence_seen
                    .get(user)
                    .is_none_or(|at| now.duration_since(*at) >= PRESENCE_REUSE)
            })
            .collect();
        if due.is_empty() {
            return;
        }
        for user in &due {
            self.presence_seen.insert(user.clone(), now);
        }
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.presences(&due).await {
                Ok(presence) => out.send(Event::Presence(presence)),
                Err(error) => log::info!("presence unavailable: {error}"),
            }
        });
    }

    /// Fetches people whose presence Graph said changed. Unlike a look, a
    /// change notice is always worth one fetch.
    fn presence_changed(&mut self, users: Vec<String>) {
        let Some(account) = &self.account else { return };
        if users.is_empty() || !account.graph.session().has_scope("Presence.Read.All") {
            return;
        }
        let now = Instant::now();
        for user in &users {
            self.presence_seen.insert(user.clone(), now);
        }
        log::debug!(
            "push[{}]: fetching presence for {} people",
            account_tag(&account.record.id),
            users.len()
        );
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.presences(&users).await {
                Ok(presence) => out.send(Event::Presence(presence)),
                Err(error) => log::info!("presence unavailable: {error}"),
            }
        });
    }

    /// The signed-in person's user id.
    fn me_id(&self) -> Option<String> {
        self.account
            .as_ref()
            .map(|account| account.record.user_id().to_owned())
    }

    /// Looks up a name once per person every ten minutes, like photos.
    fn user_name(&mut self, user: String) {
        let Some(account) = &self.account else { return };
        let now = Instant::now();
        if self
            .names_asked
            .get(&user)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(600))
        {
            return;
        }
        self.names_asked.insert(user.clone(), now);
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.user_name(&user).await {
                Ok(name) => out.send(Event::UserName { user, name }),
                Err(error) => log::debug!("name unavailable: {error}"),
            }
        });
    }

    fn photo(&mut self, user: String) {
        let Some(account) = &self.account else { return };
        let now = Instant::now();
        if self
            .photos_asked
            .get(&user)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(600))
        {
            return;
        }
        self.photos_asked.insert(user.clone(), now);
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let downloads = Arc::clone(&self.downloads);
        tokio::spawn(async move {
            let cached = {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                store.photo(&user, PHOTO_MAX_AGE)
            };
            if let Some(bytes) = cached {
                out.send(Event::Photo { user, bytes });
                return;
            }
            let Ok(_permit) = downloads.acquire().await else {
                return;
            };
            match graph.photo(&user).await {
                Ok(bytes) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = store.save_photo(&user, bytes.as_deref());
                    }
                    out.send(Event::Photo { user, bytes });
                }
                Err(error) => log::debug!("photo unavailable: {error}"),
            }
        });
    }

    fn cached_messages(&self, conversation: ConversationId) {
        let Some(account) = &self.account else { return };
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        tokio::spawn(async move {
            let key = conversation.clone();
            let messages = tokio::task::spawn_blocking(move || {
                store
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .messages(&key, CACHED_MESSAGES)
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            if !messages.is_empty() {
                out.send(Event::Messages {
                    conversation,
                    messages,
                    merge: Merge::Latest,
                    more: Some(true),
                    cached: true,
                });
            }
        });
    }

    /// Answers an agent from the cache. Never fetches.
    fn agent_read(
        &self,
        plugin: String,
        request: u64,
        query: crate::plugin::agent::AgentQuery,
        access: crate::agents::access::AgentAccess,
    ) {
        let out = self.out.clone();
        let Some(account) = &self.account else {
            out.send(Event::AgentAnswer {
                plugin,
                request,
                data: Err("not_found"),
            });
            return;
        };
        let store = Arc::clone(&account.store);
        let me = account.record.user_id();
        let me = me.to_owned();
        tokio::spawn(async move {
            let data = super::agents::answer(store, me, query, access).await;
            out.send(Event::AgentAnswer {
                plugin,
                request,
                data,
            });
        });
    }

    fn media(&mut self, url: String) {
        let Some(account) = &self.account else { return };
        if !account.graph.is_graph_url(&url) {
            // Images hosted elsewhere are not fetched with credentials, and
            // not fetched at all: they could track who read the message.
            self.out.send(Event::Media { url, bytes: None });
            return;
        }
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let downloads = Arc::clone(&self.downloads);
        tokio::spawn(async move {
            let cache = Arc::clone(&store);
            let key = url.clone();
            let cached = tokio::task::spawn_blocking(move || {
                cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .media(&key)
                    .ok()
                    .flatten()
            })
            .await
            .ok()
            .flatten();
            if let Some(bytes) = cached {
                out.send(Event::Media {
                    url,
                    bytes: Some(bytes),
                });
                return;
            }
            let Ok(_permit) = downloads.acquire().await else {
                return;
            };
            let bytes = graph
                .get_bytes(&url, crate::graph::api::MEDIA_LIMIT)
                .await
                .ok();
            if let Some(bytes) = &bytes {
                let cache = Arc::clone(&store);
                let key = url.clone();
                let encoded = bytes.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = cache
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .save_media(&key, &encoded);
                })
                .await;
            }
            out.send(Event::Media { url, bytes });
        });
    }

    fn search_in(&mut self, conversation: ConversationId, query: String, remote: bool) {
        let Some(account) = &self.account else { return };
        let store = Arc::clone(&account.store);
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            let local = {
                let store = Arc::clone(&store);
                let conversation = conversation.clone();
                let query = query.clone();
                tokio::task::spawn_blocking(move || {
                    let store = store.lock().unwrap_or_else(|p| p.into_inner());
                    store
                        .search_in(&conversation, &query, 200)
                        .unwrap_or_default()
                })
                .await
                .unwrap_or_default()
            };
            if !remote {
                out.send(Event::ConversationHits {
                    conversation,
                    query,
                    hits: local,
                });
                return;
            }
            if query.trim().chars().count() < 3 {
                return;
            }
            match graph.search_messages(&query).await {
                Ok(remote) => out.send(Event::ConversationHits {
                    hits: conversation_hits(&conversation, &local, remote),
                    conversation,
                    query,
                }),
                Err(error) => log::info!("Microsoft Search unavailable: {error}"),
            }
        });
    }

    fn search(&mut self, query: String) {
        let Some(account) = &self.account else { return };
        let store = Arc::clone(&account.store);
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            let local = {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                store.search(&query, 50).unwrap_or_default()
            };
            out.send(Event::SearchResults {
                query: query.clone(),
                hits: local,
                remote: false,
            });
            if query.trim().chars().count() < 3 {
                return;
            }
            match graph.search_messages(&query).await {
                Ok(hits) => out.send(Event::SearchResults {
                    query,
                    hits,
                    remote: true,
                }),
                Err(error) => log::info!("Microsoft Search unavailable: {error}"),
            }
        });
    }
}

/// What a conversation's find bar shows from Microsoft Search: only hits in
/// that conversation, and none already found locally.
fn conversation_hits(
    conversation: &ConversationId,
    local: &[SearchHit],
    remote: Vec<SearchHit>,
) -> Vec<SearchHit> {
    let mut hits = local.to_vec();
    for hit in remote {
        if &hit.conversation == conversation
            && !hits.iter().any(|known| known.message_id == hit.message_id)
        {
            hits.push(hit);
        }
    }
    hits
}

fn channel_access(session: &Session) -> Access {
    if session.has_scope(CHANNEL_READ_SCOPE) {
        Access::Granted
    } else {
        Access::NeedsConsent
    }
}

/// A failure to reach Microsoft at all, as opposed to an answer refusing the
/// request.
fn is_unreachable(error: &GraphError) -> bool {
    matches!(
        error,
        GraphError::Network(_) | GraphError::Auth(TokenError::Unreachable(_))
    )
}

fn describe(resource: &Resource) -> &'static str {
    match resource {
        Resource::Profile => "Loading your profile",
        Resource::Chats => "Loading chats",
        Resource::Messages(_) => "Loading messages",
        Resource::Teams => "Loading teams",
        Resource::Channels(_) => "Loading channels",
        Resource::Calendar => "Loading the calendar",
        Resource::MyPresence => "Loading your status",
        Resource::OutOfOffice => "Loading your automatic replies",
        Resource::Members(_) => "Loading the people in this chat",
        Resource::Pins(_) => "Loading the pinned messages",
    }
}

/// Whose presence the chat list shows, from the cached list: the people of
/// one-on-one chats, then the members of group chats, each most recent chat
/// first, without repeats. Meeting chats are left out: their attendee lists
/// are long and short-lived. Graph expands at most 25 members per chat in
/// the list, so larger groups are partly covered until the chat opens.
async fn contacts_of(store: &Arc<Mutex<Store>>, me: String, tenant: String) -> People {
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || {
        let chats = store
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .chats()
            .unwrap_or_default();
        people_to_follow(&chats, &me, &tenant)
    })
    .await
    .unwrap_or_default()
}

/// Whose presence the chat list shows, in order (see [`contacts_of`]).
#[derive(Debug, Default, PartialEq)]
struct People {
    /// Everyone: fetched when the person looks.
    all: Vec<String>,
    /// Those in the person's own organisation, for the presence
    /// subscription. Graph refuses (400) a subscription that includes people
    /// from other organisations, though it answers presence fetches for
    /// them. Members whose organisation Graph didn't give are kept.
    followable: Vec<String>,
}

fn people_to_follow(chats: &[crate::model::Chat], me: &str, tenant: &str) -> People {
    use crate::model::{ChatKind, Member};
    let mut people = People::default();
    let mut add = |member: &Member| {
        let user = member.user_id.as_str();
        // A deleted account stays in old chats with its name and email
        // blanked; it has no presence, and Graph refuses to follow it.
        let deleted = member.display_name.trim().is_empty() && member.email.is_none();
        if user == me || user.is_empty() || deleted || people.all.iter().any(|p| p == user) {
            return;
        }
        people.all.push(user.to_owned());
        let ours = member
            .tenant_id
            .as_deref()
            .is_none_or(|theirs| tenant.is_empty() || theirs.eq_ignore_ascii_case(tenant));
        if ours {
            people.followable.push(user.to_owned());
        }
    };
    for chat in chats {
        if let Some(peer) = chat.peer(me) {
            add(peer);
        }
    }
    for chat in chats.iter().filter(|chat| chat.kind == ChatKind::Group) {
        for member in &chat.members {
            add(member);
        }
    }
    people
}

async fn emit_cache(store: &Arc<Mutex<Store>>, out: &Outbox) {
    let store = Arc::clone(store);
    let cached = tokio::task::spawn_blocking(move || {
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        (
            store.chats().unwrap_or_default(),
            store.teams().unwrap_or_default(),
            store.channels().unwrap_or_default(),
            store.meetings().unwrap_or_default(),
        )
    })
    .await;
    let Ok((chats, teams, channels, meetings)) = cached else {
        return;
    };
    out.send(Event::Chats {
        chats,
        cached: true,
    });
    out.send(Event::Teams {
        teams,
        cached: true,
    });
    out.send(Event::Channels {
        team: None,
        channels,
    });
    out.send(Event::Meetings {
        meetings,
        cached: true,
    });
}

/// Drops a confirmed message from the outbox.
async fn forget_outbox(store: &Arc<Mutex<Store>>, local_id: &str) {
    let store = Arc::clone(store);
    let local_id = local_id.to_owned();
    let _ = tokio::task::spawn_blocking(move || {
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let _ = store.delete_outbox(&local_id);
    })
    .await;
}

/// The drafts and unconfirmed messages saved before the last exit.
async fn emit_restored(store: &Arc<Mutex<Store>>, out: &Outbox) {
    let store = Arc::clone(store);
    let saved = tokio::task::spawn_blocking(move || {
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        (
            store.drafts().unwrap_or_default(),
            store.outbox().unwrap_or_default(),
        )
    })
    .await;
    if let Ok((drafts, outbox)) = saved {
        out.send(Event::Restored { drafts, outbox });
    }
}

async fn save(store: &Arc<Mutex<Store>>, conversation: &ConversationId, messages: &[Message]) {
    if messages.is_empty() {
        return;
    }
    let store = Arc::clone(store);
    let conversation = conversation.clone();
    let messages = messages.to_vec();
    let _ = tokio::task::spawn_blocking(move || {
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(error) = store.save_messages(&conversation, &messages) {
            log::warn!("could not cache messages: {error}");
        }
    })
    .await;
}

fn download_update(
    out: &Outbox,
    updater: &fastframe_update::Updater,
    release: &fastframe_update::Release,
) -> anyhow::Result<()> {
    out.send(Event::Update(crate::updates::Event::Status(
        crate::updates::Status::Downloading {
            received: 0,
            total: 0,
        },
    )));
    let mut last_percent = 0;
    let prepared = updater.download(release, |received, total| {
        let percent = received.saturating_mul(100).checked_div(total).unwrap_or(0);
        if percent > last_percent || received == total {
            last_percent = percent;
            out.send(Event::Update(crate::updates::Event::Status(
                crate::updates::Status::Downloading { received, total },
            )));
        }
    })?;
    out.send(Event::Update(crate::updates::Event::Status(
        crate::updates::Status::Ready(Box::new(prepared)),
    )));
    Ok(())
}

fn remember_link(
    links: &Arc<Mutex<HashMap<ConversationId, String>>>,
    conversation: &ConversationId,
    link: Option<String>,
) {
    let mut links = links.lock().unwrap_or_else(|p| p.into_inner());
    match link {
        Some(link) => {
            links.insert(conversation.clone(), link);
        }
        None => {
            links.remove(conversation);
        }
    }
}

async fn fetch_resource(
    graph: &Arc<Graph>,
    store: &Arc<Mutex<Store>>,
    out: &Outbox,
    resource: &Resource,
    record: &AccountRecord,
    links: &Arc<Mutex<HashMap<ConversationId, String>>>,
) -> Result<(), GraphError> {
    match resource {
        Resource::Profile => {
            let profile = graph.me(Some(record.tenant_id.clone())).await?;
            out.send(Event::Profile(Box::new(profile)));
        }
        Resource::Chats => {
            let mut chats = graph.chats(2).await?;
            // The notes chat is not in the listing; a failure to find it
            // leaves the rest of the list as it is.
            let me = crate::model::Member {
                tenant_id: None,
                user_id: record.user_id().to_owned(),
                display_name: record.display_name.clone(),
                email: Some(record.username.clone()).filter(|u| !u.is_empty()),
            };
            match graph.notes_chat(me).await {
                Ok(Some(notes)) => chats.insert(0, notes),
                Ok(None) => {}
                Err(error) => log::info!("notes chat unavailable: {error}"),
            }
            {
                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                if let Err(error) = store.save_chats(&chats) {
                    log::warn!("could not cache chats: {error}");
                }
            }
            out.send(Event::Chats {
                chats,
                cached: false,
            });
        }
        Resource::Messages(conversation) => {
            // What the cache has, at once.
            let (cached, since) = {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                (
                    store
                        .messages(conversation, CACHED_MESSAGES)
                        .unwrap_or_default(),
                    store.last_modified(conversation),
                )
            };
            let had_cache = !cached.is_empty();
            if had_cache {
                remember_link(links, conversation, None);
                out.send(Event::Messages {
                    conversation: conversation.clone(),
                    messages: cached,
                    merge: Merge::Latest,
                    // A date-ranged refresh below cannot establish whether
                    // history before the cache exists. Keep paging available.
                    more: Some(true),
                    cached: true,
                });
            }
            match conversation {
                ConversationId::Chat(chat) => {
                    // A date-ranged refresh when the cache knows where it
                    // stopped; the newest page otherwise.
                    if let Some(since) = since.filter(|_| had_cache) {
                        let changes = graph.chat_changes(chat, since - 1_000).await?;
                        if changes.len() < crate::graph::api::PAGE * 4 {
                            save(store, conversation, &changes).await;
                            out.send(Event::Messages {
                                conversation: conversation.clone(),
                                messages: changes,
                                merge: Merge::Changes,
                                more: None,
                                cached: false,
                            });
                            return Ok(());
                        }
                    }
                    let page = graph.chat_messages(chat, None).await?;
                    remember_link(links, conversation, page.older.clone());
                    let more =
                        page.older.is_some() || page.messages.len() >= crate::graph::api::PAGE;
                    save(store, conversation, &page.messages).await;
                    out.send(Event::Messages {
                        conversation: conversation.clone(),
                        messages: page.messages,
                        merge: Merge::Latest,
                        more: Some(more),
                        cached: false,
                    });
                }
                ConversationId::Channel { team, channel } => {
                    let page = graph.channel_messages(team, channel, None).await?;
                    out.send(Event::ChannelAccess(Access::Granted));
                    remember_link(links, conversation, page.older.clone());
                    save(store, conversation, &page.messages).await;
                    out.send(Event::Messages {
                        conversation: conversation.clone(),
                        messages: page.messages,
                        merge: Merge::Latest,
                        more: Some(page.older.is_some()),
                        cached: false,
                    });
                }
            }
        }
        Resource::Teams => {
            let teams = graph.joined_teams().await?;
            {
                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                let _ = store.save_teams(&teams);
            }
            out.send(Event::Teams {
                teams: teams.clone(),
                cached: false,
            });
            // Channels of every team, a few at a time.
            let limit = Arc::new(Semaphore::new(3));
            let mut tasks = Vec::new();
            for team in teams {
                let graph = Arc::clone(graph);
                let store = Arc::clone(store);
                let out = out.clone();
                let limit = Arc::clone(&limit);
                tasks.push(tokio::spawn(async move {
                    let Ok(_permit) = limit.acquire().await else {
                        return;
                    };
                    match graph.channels(&team.id).await {
                        Ok(channels) => {
                            {
                                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                                let _ = store.save_channels(&team.id, &channels);
                            }
                            out.send(Event::Channels {
                                team: Some(team.id),
                                channels,
                            });
                        }
                        Err(error) => log::info!("channels unavailable for a team: {error}"),
                    }
                }));
            }
            for task in tasks {
                let _ = task.await;
            }
        }
        Resource::Channels(team) => {
            let channels = graph.channels(team).await?;
            {
                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                let _ = store.save_channels(team, &channels);
            }
            out.send(Event::Channels {
                team: Some(team.clone()),
                channels,
            });
        }
        Resource::Calendar => {
            let from = crate::time::start_of_day(crate::time::now());
            let to = from + 14 * 24 * 60 * 60 * 1000;
            let meetings = graph.calendar(from, to).await?;
            {
                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                let _ = store.save_meetings(&meetings);
            }
            out.send(Event::Meetings {
                meetings,
                cached: false,
            });
        }
        Resource::MyPresence => {
            let presence = graph.my_presence().await?;
            out.send(Event::MyPresence(presence));
        }
        Resource::Pins(chat) => {
            let me = record.user_id();
            let pins = graph.pinned_messages(chat, me).await?;
            out.send(Event::Pins {
                chat: chat.clone(),
                pins,
            });
        }
        Resource::Members(chat) => {
            let members: Vec<crate::model::Member> = graph
                .chat_members(chat)
                .await?
                .into_iter()
                .map(|(_, member)| member)
                .collect();
            {
                // The cached chat keeps the whole list, not the first 25.
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(mut cached) = store
                    .chats()
                    .ok()
                    .and_then(|chats| chats.into_iter().find(|c| &c.id == chat))
                {
                    cached.members = members.clone();
                    let _ = store.upsert_chat(&cached);
                }
            }
            out.send(Event::Members {
                chat: chat.clone(),
                members,
            });
        }
        Resource::OutOfOffice => match graph.out_of_office().await {
            Ok(value) => out.send(Event::OutOfOffice(value)),
            Err(error) if crate::graph::mailbox::no_mailbox(&error) => {
                out.send(Event::OutOfOfficeUnavailable);
            }
            Err(error) => return Err(error),
        },
    }
    Ok(())
}

/// Uploads one file, shares it with the conversation, and returns the
/// reference for the message. Sharing problems are reported but do not stop
/// the send: the people may already have access.
async fn share_file(
    graph: &Graph,
    conversation: &ConversationId,
    path: &std::path::Path,
    recipients: &[String],
    out: &Outbox,
) -> Result<SharedFile, GraphError> {
    let item = graph.upload_file(path).await?;
    let shared = match conversation {
        ConversationId::Chat(_) => graph.share_with(&item.id, recipients).await,
        ConversationId::Channel { .. } => graph.organization_link(&item.id).await,
    };
    if let Err(error) = shared {
        log::warn!("sharing an upload failed: {error}");
        out.send(Event::Error(format!(
            "{} was uploaded, but sharing it failed; some people may not be able to open it ({error})",
            item.name
        )));
    }
    let (Some(id), Some(url)) = (item.attachment_id(), item.content_url().map(str::to_owned))
    else {
        return Err(GraphError::Decode);
    };
    Ok(SharedFile {
        id,
        name: item.name,
        url,
    })
}

/// Keeps one subscription alive while `stop` stays false: connect to the
/// relay, subscribe, forward notices, renew, and start over after failures
/// with growing pauses.
/// Where a push loop connects, for whom, and where it records the
/// subscriptions it makes.
struct PushTarget {
    me: String,
    source: PushSource,
    leftovers: std::path::PathBuf,
    /// The people of one-on-one chats, as the chat list last showed them.
    contacts: watch::Receiver<Vec<String>>,
    /// Where people Graph refused to follow are remembered.
    refused: std::path::PathBuf,
    /// How often the session renews its subscriptions.
    renew_every: Duration,
    /// How long the WebSocket to the relay may take to open.
    connect_within: Duration,
}

/// How the push loop hears from Graph.
enum PushSource {
    /// A relay reached over a WebSocket, with its access key if it has one.
    Relay { relay: String, key: Option<String> },
    /// A push plugin: Graph posts to the plugin's tunnel and the plugin
    /// forwards each body through the queue.
    Plugin {
        route: super::PushRoute,
        bodies: mpsc::Receiver<String>,
    },
}

/// One thing a push session received.
enum Incoming {
    Text(String),
    Skip,
    Closed(&'static str),
}

/// People Graph refused to follow in the last week (see
/// [`realtime::REFUSED_FILE`]).
async fn load_refused(path: &std::path::Path) -> HashMap<String, i64> {
    let path = path.to_owned();
    let now = crate::time::now();
    tokio::task::spawn_blocking(move || realtime::read_refused(&path, now))
        .await
        .unwrap_or_default()
}

/// Saves newly refused people, so later sessions leave them out at once.
async fn save_refused(path: &std::path::Path, limits: &mut PresenceLimits) {
    if !limits.refused_changed {
        return;
    }
    limits.refused_changed = false;
    let path = path.to_owned();
    let refused = limits.refused.clone();
    let written =
        tokio::task::spawn_blocking(move || realtime::write_refused(&path, &refused)).await;
    if !matches!(written, Ok(Ok(()))) {
        push_log!(warn, "could not remember refused presence users");
    }
}

/// What the person reads when a sign-in could not be kept for next time.
fn unsaved_sign_in_text(locked: bool) -> &'static str {
    if locked {
        "Chaat could not save this sign-in because the system keyring is locked. Unlock it and sign in again, or Chaat will ask you to sign in next time."
    } else {
        "Chaat could not save this sign-in to the system credential store, so it will ask you to sign in again next time."
    }
}

/// Where Graph sends a session's notifications.
struct Endpoints {
    notification: String,
    lifecycle: String,
    client_state: String,
}

/// The presence subscription and whom it follows.
struct Followed {
    id: String,
    people: Vec<String>,
}

/// The next list size to try when Graph accepted `lo` people (0: none yet)
/// and refused `hi` as malformed: halve until a size works, then bisect the
/// gap. `None` when `lo` is the most Graph takes.
fn next_presence_size(lo: usize, hi: usize) -> Option<usize> {
    if hi <= lo + 1 {
        None
    } else if lo == 0 {
        Some(hi / 2)
    } else {
        Some((lo + hi) / 2)
    }
}

/// What a push session learned about following presence, kept until push
/// reconnects so chat-list updates don't repeat refused requests.
struct PresenceLimits {
    /// The most people to ask for (0: don't try again this session).
    cap: usize,
    /// People Graph said it cannot follow (lower-case id → when), from
    /// earlier sessions too ([`realtime::REFUSED_FILE`]).
    refused: HashMap<String, i64>,
    /// Whether `refused` gained someone since it was last saved.
    refused_changed: bool,
    /// Graph answered 409 (an earlier presence subscription still holds the
    /// person's one place): try again when push next renews.
    blocked: bool,
}

impl PresenceLimits {
    fn new() -> Self {
        Self {
            cap: realtime::PRESENCE_LIMIT,
            refused: HashMap::new(),
            refused_changed: false,
            blocked: false,
        }
    }

    /// `people` without those Graph refused, up to the cap.
    fn wanted(&self, people: &[String]) -> Vec<String> {
        people
            .iter()
            .filter(|id| !self.refused.contains_key(&id.to_ascii_lowercase()))
            .take(self.cap)
            .cloned()
            .collect()
    }
}

/// How many rounds of "remove the failing users" to follow before searching.
const REFUSAL_ROUNDS: usize = 5;

/// Subscribes to the presence of `people` (the person first, then the most
/// wanted contacts). Graph refuses (400) a list containing users it cannot
/// follow and names them; those are dropped, remembered, and the rest asked
/// for again. A 400 that names nobody falls back to searching for the
/// longest accepted list (see [`search_presence`]). A conflict (409) is
/// retried once after a pause, since Graph can take a moment to forget a
/// subscription just deleted; other refusals end attempts for the session.
async fn follow_presence(
    graph: &Graph,
    people: &[String],
    to: &Endpoints,
    limits: &mut PresenceLimits,
) -> Option<Followed> {
    let mut list = limits.wanted(people);
    let mut conflict_retried = false;
    let mut rounds = 0;
    loop {
        if list.is_empty() {
            limits.cap = 0;
            return None;
        }
        match subscribe_presence_of(graph, &list, to).await {
            Ok(id) => {
                push_log!(
                    info,
                    "following presence of {} of {} people",
                    list.len(),
                    people.len()
                );
                return Some(Followed { id, people: list });
            }
            Err(GraphError::UsersRefused(ids)) if rounds < REFUSAL_ROUNDS => {
                rounds += 1;
                push_log!(
                    info,
                    "Graph cannot follow {} people; leaving them out",
                    ids.len()
                );
                let now = crate::time::now();
                limits
                    .refused
                    .extend(ids.iter().map(|id| (id.to_ascii_lowercase(), now)));
                limits.refused_changed = true;
                list = limits.wanted(people);
            }
            Err(GraphError::UsersRefused(_) | GraphError::Http { status: 400, .. }) => {
                return search_presence(graph, &list, people.len(), to, limits).await;
            }
            Err(GraphError::Http { status: 409, .. }) if !conflict_retried => {
                conflict_retried = true;
                let cleared = clear_presence_blockers(graph).await;
                push_log!(
                    info,
                    "presence subscription conflict (409); deleted {cleared} in the way, retrying"
                );
                if cleared == 0 {
                    // Graph may still be forgetting one just deleted.
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
            Err(GraphError::Http { status: 409, .. }) => {
                push_log!(
                    info,
                    "presence subscription conflict (409); trying again at the next renewal"
                );
                limits.cap = 0;
                limits.blocked = true;
                return None;
            }
            Err(error) => {
                push_log!(
                    info,
                    "no presence subscription for {} people ({error})",
                    list.len()
                );
                limits.cap = 0;
                return None;
            }
        }
    }
}

/// Deletes this app's presence subscriptions for the person, which hold the
/// one place Graph allows (a run that crashed or was killed before cleaning
/// up). Callers hold none of their own when they ask. Chaat runs on one
/// device per person; a second one would lose its presence subscription
/// here and fall back to fetching on look. Returns how many were deleted.
async fn clear_presence_blockers(graph: &Graph) -> usize {
    let Ok(subscriptions) = graph.subscriptions().await else {
        return 0;
    };
    let mut deleted = 0;
    for subscription in subscriptions.iter().filter(|s| {
        s.resource.as_deref().is_some_and(|resource| {
            resource
                .to_ascii_lowercase()
                .contains("communications/presences")
        })
    }) {
        if graph.delete_subscription(&subscription.id).await.is_ok() {
            deleted += 1;
        }
    }
    deleted
}

async fn subscribe_presence_of(
    graph: &Graph,
    people: &[String],
    to: &Endpoints,
) -> Result<String, GraphError> {
    graph
        .subscribe_presence(
            people,
            &to.notification,
            &to.lifecycle,
            &to.client_state,
            realtime::LIFETIME,
        )
        .await
        .map(|subscription| subscription.id)
}

/// Finds the longest prefix of `people` Graph accepts when it refuses a list
/// without saying whom it objects to: halving until a size works, then
/// bisecting. Graph allows one presence subscription per person and app, so
/// trying a longer list first deletes the one that works, and a refusal
/// recreates it. Keeps the size found in `limits.cap` for the session.
async fn search_presence(
    graph: &Graph,
    people: &[String],
    asked: usize,
    to: &Endpoints,
    limits: &mut PresenceLimits,
) -> Option<Followed> {
    let total = people.len();
    let mut lo = 0;
    let mut hi = total;
    let mut held: Option<Followed> = None;
    let mut size = total / 2;
    while size > 0 {
        if let Some(old) = held.take() {
            let _ = graph.delete_subscription(&old.id).await;
        }
        match subscribe_presence_of(graph, &people[..size], to).await {
            Ok(id) => {
                lo = size;
                held = Some(Followed {
                    id,
                    people: people[..size].to_vec(),
                });
            }
            Err(GraphError::UsersRefused(_) | GraphError::Http { status: 400, .. }) => {
                push_log!(
                    info,
                    "presence subscription for {size} people refused (400)"
                );
                hi = size;
            }
            Err(error) => {
                push_log!(
                    info,
                    "presence subscription for {size} people failed ({error})"
                );
                hi = size;
                if lo == 0 {
                    break;
                }
            }
        }
        match next_presence_size(lo, hi) {
            Some(next) => size = next,
            None => break,
        }
    }
    if lo == 0 {
        limits.cap = 0;
        return None;
    }
    if held.is_none() {
        // The last, longer try was refused after the working one was deleted.
        match subscribe_presence_of(graph, &people[..lo], to).await {
            Ok(id) => {
                held = Some(Followed {
                    id,
                    people: people[..lo].to_vec(),
                })
            }
            Err(error) => {
                push_log!(info, "no presence subscription for {lo} people ({error})");
                limits.cap = 0;
                return None;
            }
        }
    }
    push_log!(info, "following presence of {lo} of {asked} people");
    limits.cap = lo;
    held
}

/// The ids of the subscriptions a session holds.
fn held(chat: &str, presence: Option<&Followed>) -> Vec<String> {
    std::iter::once(chat.to_owned())
        .chain(presence.map(|p| p.id.clone()))
        .collect()
}

/// Records the subscriptions a session holds, so a run that ends without
/// cleaning up can be cleaned up by the next one.
async fn record_subscriptions(path: &std::path::Path, ids: &[String]) {
    let path = path.to_owned();
    let ids = ids.to_vec();
    let written = tokio::task::spawn_blocking(move || realtime::write_leftovers(&path, &ids)).await;
    if !matches!(written, Ok(Ok(()))) {
        push_log!(warn, "could not record the subscriptions");
    }
}

/// Deletes subscriptions an earlier run made and never deleted. Best effort:
/// any that remain expire within the hour.
async fn delete_leftovers(graph: &Graph, path: &std::path::Path) {
    let read = path.to_owned();
    let ids = tokio::task::spawn_blocking(move || realtime::read_leftovers(&read))
        .await
        .unwrap_or_default();
    if ids.is_empty() {
        return;
    }
    let mut deleted = 0;
    for id in &ids {
        if graph.delete_subscription(id).await.is_ok() {
            deleted += 1;
        }
    }
    push_log!(
        info,
        "deleted {deleted} of {} subscriptions left by an earlier run",
        ids.len()
    );
    record_subscriptions(path, &[]).await;
}

async fn push_loop(
    graph: Arc<Graph>,
    target: PushTarget,
    out: Outbox,
    internal: mpsc::UnboundedSender<Internal>,
    mut stop: watch::Receiver<bool>,
) {
    use futures_util::{SinkExt, StreamExt};
    let PushTarget {
        me,
        source,
        leftovers,
        mut contacts,
        refused,
        renew_every,
        connect_within,
    } = target;
    let (relay_key, mut bodies, relay, fixed_channel) = match source {
        PushSource::Relay { relay, key } => (Some(key), None, relay, None),
        PushSource::Plugin { route, bodies } => {
            (None, Some(bodies), route.url, Some(route.channel))
        }
    };
    let mut pause = Duration::from_secs(5);
    while !*stop.borrow() {
        out.send(Event::Push(PushStatus::Connecting));
        let channel = fixed_channel
            .clone()
            .unwrap_or_else(|| auth::random_token(32));
        let client_state = auth::random_token(24);
        let mut live_at: Option<Instant> = None;
        let failure = 'session: {
            let mut sink = None;
            let mut stream = None;
            if let Some(key) = &relay_key {
                let Some(request) = realtime::websocket_request(&relay, &channel, key.as_deref())
                else {
                    break 'session "the relay address or access key is not valid".to_owned();
                };
                let connecting =
                    tokio::time::timeout(connect_within, tokio_tungstenite::connect_async(request))
                        .await;
                let Ok(connected) = connecting else {
                    break 'session "the relay did not answer in time".to_owned();
                };
                let socket = match connected {
                    Ok((socket, _)) => socket,
                    Err(tokio_tungstenite::tungstenite::Error::Http(response))
                        if response.status() == 401 =>
                    {
                        break 'session if key.is_some() {
                            "the relay did not accept the access key".to_owned()
                        } else {
                            "the relay needs an access key".to_owned()
                        };
                    }
                    Err(_) => break 'session "the relay could not be reached".to_owned(),
                };
                let (write, read) = socket.split();
                sink = Some(write);
                stream = Some(read);
            }
            delete_leftovers(&graph, &leftovers).await;
            let subscription = match graph
                .subscribe_chat_messages(
                    &me,
                    &realtime::notification_url(&relay, &channel),
                    &realtime::lifecycle_url(&relay, &channel),
                    &client_state,
                    realtime::LIFETIME,
                )
                .await
            {
                Ok(subscription) => subscription,
                Err(error) => {
                    break 'session format!("Microsoft Graph refused the subscription ({error})");
                }
            };
            // Presence: the person's own, so the call bar follows calls, and
            // their one-on-one contacts', for the dots. Optional: chat updates
            // still work without it.
            let endpoints = Endpoints {
                notification: realtime::notification_url(&relay, &channel),
                lifecycle: realtime::lifecycle_url(&relay, &channel),
                client_state: client_state.clone(),
            };
            let wanted = realtime::presence_watch(&me, &contacts.borrow_and_update());
            let mut limits = PresenceLimits::new();
            limits.refused = load_refused(&refused).await;
            let mut presence = follow_presence(&graph, &wanted, &endpoints, &mut limits).await;
            save_refused(&refused, &mut limits).await;
            let mut subscriptions = held(&subscription.id, presence.as_ref());
            record_subscriptions(&leftovers, &subscriptions).await;
            let mut contacts_open = true;
            out.send(Event::Push(PushStatus::Live));
            live_at = Some(Instant::now());
            let mut reauthorized: HashMap<String, Instant> = HashMap::new();
            // A change may have happened while connecting.
            let _ = internal.send(Internal::PushMissed);
            let mut renew = tokio::time::interval(renew_every);
            renew.tick().await;
            let mut keepalive = tokio::time::interval(Duration::from_secs(30));
            keepalive.tick().await;
            let reason = loop {
                tokio::select! {
                    changed = stop.changed() => {
                        if changed.is_err() || *stop.borrow() {
                            break None;
                        }
                    }
                    changed = contacts.changed(), if contacts_open => {
                        if changed.is_err() {
                            contacts_open = false;
                            continue;
                        }
                        let wanted = realtime::presence_watch(&me, &contacts.borrow_and_update());
                        let wanted = limits.wanted(&wanted);
                        let followed = presence.as_ref().map_or(&[][..], |p| p.people.as_slice());
                        // Graph cannot change whom a subscription follows,
                        // and allows one presence subscription per person
                        // and app: a new contact means replacing it.
                        if wanted.iter().any(|id| !followed.contains(id)) {
                            if let Some(old) = presence.take() {
                                let _ = graph.delete_subscription(&old.id).await;
                            }
                            presence = follow_presence(&graph, &wanted, &endpoints, &mut limits).await;
                            save_refused(&refused, &mut limits).await;
                            subscriptions = held(&subscription.id, presence.as_ref());
                            record_subscriptions(&leftovers, &subscriptions).await;
                        }
                    }
                    _ = renew.tick() => {
                        let mut failed = None;
                        for id in &subscriptions {
                            if let Err(error) = graph.renew_subscription(id, realtime::LIFETIME).await {
                                failed = Some(format!("renewing the subscription failed ({error})"));
                            }
                        }
                        if failed.is_some() {
                            break failed;
                        }
                        // A presence subscription Graph refused as a
                        // duplicate gets one more try now; the one in the
                        // way, if abandoned, expires within the hour.
                        if presence.is_none() && limits.blocked {
                            limits.blocked = false;
                            limits.cap = realtime::PRESENCE_LIMIT;
                            let wanted = realtime::presence_watch(&me, &contacts.borrow());
                            presence = follow_presence(&graph, &wanted, &endpoints, &mut limits).await;
                            save_refused(&refused, &mut limits).await;
                            subscriptions = held(&subscription.id, presence.as_ref());
                            record_subscriptions(&leftovers, &subscriptions).await;
                        }
                    }
                    _ = keepalive.tick() => {
                        if let Some(sink) = sink.as_mut()
                            && sink.send(tokio_tungstenite::tungstenite::Message::Ping(Vec::new().into())).await.is_err()
                        {
                            break Some("the relay connection dropped".to_owned());
                        }
                    }
                    incoming = async {
                        if let Some(stream) = stream.as_mut() {
                            match stream.next().await {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => Incoming::Text(text.to_string()),
                                Some(Ok(_)) => Incoming::Skip,
                                Some(Err(_)) | None => Incoming::Closed("the relay connection dropped"),
                            }
                        } else if let Some(bodies) = bodies.as_mut() {
                            match bodies.recv().await {
                                Some(text) => Incoming::Text(text),
                                None => Incoming::Closed("the push plugin stopped"),
                            }
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        let text = match incoming {
                            Incoming::Text(text) => text,
                            Incoming::Skip => continue,
                            Incoming::Closed(reason) => break Some(reason.to_owned()),
                        };
                        let mut recreate = false;
                        for notice in realtime::parse(&text, &client_state) {
                            match notice {
                                Notice::Chat(chat) => {
                                    push_log!(debug,"chat change notice");
                                    let _ = internal.send(Internal::Pushed(chat));
                                }
                                Notice::Presence(user) => {
                                    let _ = internal.send(Internal::PresencePushed(user));
                                }
                                Notice::Missed => {
                                    let _ = internal.send(Internal::PushMissed);
                                }
                                // Lifecycle notices count only for a subscription
                                // this session holds, and a reauthorization at
                                // most once per `REAUTHORIZE_GAP`: whoever
                                // forwards bodies knows `clientState`.
                                Notice::Reauthorize(Some(id)) if subscriptions.contains(&id) => {
                                    let now = Instant::now();
                                    let recent = reauthorized
                                        .get(&id)
                                        .is_some_and(|at| now.saturating_duration_since(*at) < REAUTHORIZE_GAP);
                                    if !recent {
                                        reauthorized.insert(id.clone(), now);
                                        if graph.reauthorize_subscription(&id).await.is_err() {
                                            recreate = true;
                                        }
                                    }
                                }
                                Notice::Removed(Some(id)) if subscriptions.contains(&id) => {
                                    recreate = true;
                                }
                                Notice::Reauthorize(_) | Notice::Removed(_) => {
                                    push_log!(debug, "lifecycle notice for a subscription not held; ignored");
                                }
                            }
                        }
                        if recreate {
                            break Some("Microsoft Graph ended the subscription".to_owned());
                        }
                    }
                }
            };
            // Both at once, to keep quitting quick. A failure stays listed
            // for the next start; anything left expires within the hour.
            let deletes = subscriptions.iter().map(|id| graph.delete_subscription(id));
            let results = futures_util::future::join_all(deletes).await;
            let remaining: Vec<String> = subscriptions
                .iter()
                .zip(&results)
                .filter(|(_, result)| result.is_err())
                .map(|(id, _)| id.clone())
                .collect();
            record_subscriptions(&leftovers, &remaining).await;
            match reason {
                Some(reason) => reason,
                None => return,
            }
        };
        if *stop.borrow() {
            return;
        }
        // Only a session that stayed up a while starts the wait over: one
        // ended at once (say, by a `subscriptionRemoved` again and again)
        // keeps backing off, so subscriptions are not remade every few
        // seconds.
        if live_at.is_some_and(|at| at.elapsed() >= HEALTHY_PUSH_SESSION) {
            pause = Duration::from_secs(5);
        }
        push_log!(info, "{failure}; retrying in {pause:?}");
        out.send(Event::Push(PushStatus::Failed(failure)));
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            _ = stop.changed() => {}
        }
        pause = (pause * 2).min(Duration::from_secs(300));
    }
}

/// What a browser sign-in asks Microsoft for.
struct Ask<'a> {
    extra: &'a [String],
    /// The account signing in again, if any.
    hint: Option<&'a str>,
    /// New scopes are requested, so show the consent screen.
    consent: bool,
}

async fn browser_sign_in(
    http: &reqwest::Client,
    config: &AuthConfig,
    ask: Ask<'_>,
    out: &Outbox,
    cancelled: watch::Receiver<bool>,
    // `None`: don't open a browser (tests); `Some(None)`: the default one.
    open_browser: Option<Option<crate::calling::browser::BrowserProfile>>,
) -> Result<token::TokenResponse, String> {
    let loopback = Loopback::bind().await.map_err(|e| e.to_string())?;
    let flow = auth::begin_browser(config, loopback.port(), ask.extra, ask.hint, ask.consent);
    out.send(Event::Auth(AuthStatus::WaitingForBrowser {
        url: flow.url.clone(),
    }));
    if let Some(browser) = open_browser {
        // The call browser, so Teams on the web later finds this Microsoft
        // session; the default browser otherwise or if that fails.
        let launched = browser.and_then(|b| {
            crate::calling::browser::page(&b.browser, &flow.url, b.profile.as_deref())
                .map(|launch| crate::calling::browser::spawn(&launch))
        });
        if !matches!(launched, Some(Ok(())))
            && let Err(error) = open::that_detached(&flow.url)
        {
            log::warn!("could not open the browser: {error}");
        }
    }
    let code = loopback
        .wait_for_code(&flow.state, cancelled)
        .await
        .map_err(|e| e.to_string())?;
    out.send(Event::Auth(AuthStatus::Finishing));
    token::exchange_code(
        http,
        config,
        &code,
        &flow.verifier,
        &flow.redirect_uri,
        &flow.scopes,
    )
    .await
    .map_err(|e| e.to_string())
}

async fn device_code_sign_in(
    http: &reqwest::Client,
    config: &AuthConfig,
    extra: &[String],
    out: &Outbox,
    cancelled: watch::Receiver<bool>,
) -> Result<token::TokenResponse, String> {
    let scopes = auth::scope_string(extra);
    let code = token::start_device_code(http, config, &scopes)
        .await
        .map_err(|e| e.to_string())?;
    out.send(Event::Auth(AuthStatus::DeviceCode {
        code: code.user_code.clone(),
        url: code.verification_uri.clone(),
    }));
    let response = token::poll_device_code(http, config, &code, cancelled)
        .await
        .map_err(|e| e.to_string())?;
    out.send(Event::Auth(AuthStatus::Finishing));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Chat, ChatKind, Member};

    /// A chat whose members are `id` (in the person's organisation, "t-1"),
    /// `id@t-2` (another organisation) or `id@` (organisation not given).
    fn chat(kind: ChatKind, members: &[&str]) -> Chat {
        Chat {
            kind,
            members: members
                .iter()
                .map(|spec| {
                    let (id, tenant) = match spec.split_once('@') {
                        Some((id, "")) => (id, None),
                        Some((id, tenant)) => (id, Some(tenant.to_owned())),
                        None => (*spec, Some("T-1".to_owned())),
                    };
                    Member {
                        user_id: id.into(),
                        display_name: id.to_uppercase(),
                        email: None,
                        tenant_id: tenant,
                    }
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn the_presence_search_finds_the_longest_accepted_list() {
        let total = 66;
        for limit in 1..=total {
            let (mut lo, mut hi, mut size, mut tries) = (0, total + 1, total, 0);
            loop {
                tries += 1;
                if size <= limit {
                    lo = size;
                } else {
                    hi = size;
                }
                match next_presence_size(lo, hi) {
                    Some(next) => size = next,
                    None => break,
                }
            }
            assert_eq!(lo, limit, "limit {limit}");
            assert!(tries <= 13, "limit {limit} took {tries} tries");
        }
        // Everyone accepted at once: one try.
        assert_eq!(next_presence_size(66, 67), None);
    }

    #[test]
    fn one_on_one_contacts_come_before_group_members_and_meetings_are_left_out() {
        let chats = [
            chat(ChatKind::Group, &["me", "grace", "ada"]),
            chat(ChatKind::OneOnOne, &["me", "bo"]),
            chat(ChatKind::Meeting, &["me", "visitor"]),
            chat(ChatKind::OneOnOne, &["ada", "me"]),
            chat(ChatKind::Group, &["me", "bo", "hedy"]),
        ];
        let people = people_to_follow(&chats, "me", "t-1");
        assert_eq!(people.all, ["bo", "ada", "grace", "hedy"]);
        assert_eq!(people.followable, people.all);
    }

    #[test]
    fn deleted_accounts_are_neither_fetched_nor_followed() {
        let mut gone = chat(ChatKind::Group, &["me", "ada", "gone"]);
        gone.members[2].display_name.clear();
        let people = people_to_follow(&[gone], "me", "t-1");
        assert_eq!(people.all, ["ada"]);
        assert_eq!(people.followable, ["ada"]);
    }

    #[test]
    fn people_from_other_organisations_are_fetched_but_not_followed() {
        let chats = [
            chat(ChatKind::OneOnOne, &["me", "partner@t-2"]),
            chat(ChatKind::OneOnOne, &["me", "ada"]),
            chat(ChatKind::Group, &["me", "guest@t-3", "unknown@", "grace"]),
        ];
        let people = people_to_follow(&chats, "me", "t-1");
        assert_eq!(people.all, ["partner", "ada", "guest", "unknown", "grace"]);
        assert_eq!(people.followable, ["ada", "unknown", "grace"]);
    }
}

/// Draft saves waiting for the disk. Typing queues a save per frame; they
/// collapse to the latest one per conversation, and one blocking task writes
/// them, so the command loop never waits on SQLite.
type PendingDraft = (Arc<Mutex<Store>>, Option<crate::outbox::StoredDraft>);

#[derive(Default)]
struct DraftQueue {
    pending: Mutex<HashMap<ConversationId, PendingDraft>>,
    running: std::sync::atomic::AtomicBool,
}

impl DraftQueue {
    fn push(
        self: &Arc<Self>,
        store: &Arc<Mutex<Store>>,
        conversation: ConversationId,
        draft: Option<crate::outbox::StoredDraft>,
    ) {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(conversation, (Arc::clone(store), draft));
        if !self.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let queue = Arc::clone(self);
            tokio::task::spawn_blocking(move || queue.flush());
        }
    }

    fn flush(&self) {
        loop {
            let batch = {
                let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
                if pending.is_empty() {
                    // Cleared under the lock, so a push cannot slip between.
                    self.running
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
                std::mem::take(&mut *pending)
            };
            for (conversation, (store, draft)) in batch {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                if let Err(error) = store.save_draft(&conversation, draft.as_ref()) {
                    log::warn!("could not save a draft: {error}");
                }
            }
        }
    }
}

#[cfg(test)]
mod draft_queue_tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn latest_draft_per_conversation_wins() {
        let store = Arc::new(Mutex::new(Store::in_memory().unwrap()));
        let queue = Arc::new(DraftQueue::default());
        let chat = ConversationId::Chat("19:a".into());
        for n in 0..200 {
            queue.push(
                &store,
                chat.clone(),
                Some(crate::outbox::StoredDraft {
                    text: format!("t{n}"),
                    ..Default::default()
                }),
            );
        }
        while queue.running.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let drafts = store.lock().unwrap().drafts().unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].1.text, "t199");
        queue.push(&store, chat, None);
        while queue.running.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(store.lock().unwrap().drafts().unwrap().is_empty());
    }
}
