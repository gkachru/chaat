//! The interface's handle to the background runtime.
//!
//! A tokio runtime on its own thread owns sign-in, the Graph client, the
//! cache and every network call (`backend/worker.rs`). The interface sends
//! [`Command`]s and drains [`Event`]s each frame; every event wakes the
//! window through the shell's [`Waker`], so an idle window draws nothing.

mod setup;
mod setup_export;
mod worker;

use std::time::Duration;

use tokio::sync::mpsc;

use crate::model::{
    Access, Availability, Channel, Chat, ConversationId, Meeting, Member, MentionRef, Message,
    Millis, PinnedMessage, Presence, Profile, SearchHit, Team,
};
use crate::paths::AppDirs;
use crate::sync::{Resource, Trigger};

pub use fastframe_shell::Waker;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    /// No app registration is set up yet.
    NotConfigured,
    SignedOut,
    /// Reading the stored session at start.
    Restoring,
    /// The browser is open on Microsoft's sign-in page.
    WaitingForBrowser {
        url: String,
    },
    /// Device code sign-in: enter `code` at `url` on any device.
    DeviceCode {
        code: String,
        url: String,
    },
    /// Exchanging the code for tokens.
    Finishing,
    SignedIn {
        account: String,
        name: String,
        username: String,
    },
    /// The stored session no longer works; sign in again.
    Expired {
        username: String,
        reason: String,
    },
    Failed(String),
}

impl AuthStatus {
    /// For the log: no URLs, codes or names.
    pub fn log_label(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not configured",
            Self::SignedOut => "signed out",
            Self::Restoring => "restoring",
            Self::WaitingForBrowser { .. } => "waiting for browser",
            Self::DeviceCode { .. } => "device code",
            Self::Finishing => "finishing",
            Self::SignedIn { .. } => "signed in",
            Self::Expired { .. } => "expired",
            Self::Failed(_) => "failed",
        }
    }
}

#[derive(Debug)]
pub enum Command {
    CheckUpdate {
        download: bool,
    },
    DownloadUpdate(fastframe_update::Release),
    InstallUpdate(Box<fastframe_update::Prepared>),
    AcknowledgeUpdate(fastframe_update::Receipt),
    DiscardUpdate(Box<fastframe_update::Prepared>),
    /// The app registration and the account to restore, once at start and
    /// whenever the registration changes.
    Start {
        client_id: String,
        tenant: String,
        account: Option<String>,
    },
    SignIn {
        extra_scopes: Vec<String>,
        device_code: bool,
    },
    CancelSignIn,
    SignOut,
    Fetch {
        resource: Resource,
        trigger: Trigger,
    },
    LoadOlder {
        conversation: ConversationId,
        before: Option<i64>,
    },
    /// Reopen a model evicted from RAM without bypassing Graph freshness.
    CachedMessages(ConversationId),
    Paste {
        conversation: ConversationId,
        range: (usize, usize),
        text: String,
    },
    LoadReplies {
        conversation: ConversationId,
        root: String,
    },
    Send {
        conversation: ConversationId,
        local_id: String,
        html: String,
        /// A chat message to quote, or a channel post to reply under.
        reply_to: Option<String>,
        /// The people `<at id="n">` tags in `html` name, by index.
        mentions: Vec<MentionRef>,
        /// Files to upload to OneDrive and share in the message.
        files: Vec<std::path::PathBuf>,
        images: Vec<crate::clipboard::InlineImage>,
        /// Who to share the files with (the chat's members' addresses).
        recipients: Vec<String>,
    },
    /// Saves (or with `None` removes) the draft of one conversation.
    SaveDraft {
        conversation: ConversationId,
        draft: Option<crate::outbox::StoredDraft>,
    },
    /// Keeps a message until Graph confirms it, so a restart can resend it.
    SaveOutbox(crate::outbox::OutboxEntry),
    /// Forgets an unsent message (by its local id).
    DropOutbox(String),
    /// Sends a kept message again after a restart, once the conversation's
    /// newest page shows Graph did not already take it.
    Resend(crate::outbox::OutboxEntry),
    SearchPeople(String),
    /// Adds people to a group chat, one request each; `history` lets them
    /// see the messages from before they joined.
    AddMembers {
        chat: String,
        users: Vec<String>,
        history: bool,
    },
    /// Leaves a group chat: finds the person's own membership and deletes
    /// it (needs ChatMember.ReadWrite).
    LeaveChat(String),
    /// A one-on-one chat with one person, or a group chat with several.
    CreateChat {
        others: Vec<String>,
        topic: Option<String>,
    },
    /// Sets (`Some`) or resets (`None`) the person's preferred availability.
    SetAvailability(Option<Availability>),
    /// Sets the status message, which clears itself at `expires` (UTC
    /// milliseconds) when there is one.
    SetStatusMessage {
        text: String,
        expires: Option<Millis>,
    },
    /// Saves the automatic replies (Out of Office).
    SetOutOfOffice(crate::model::OutOfOffice),
    /// The person is using Chaat: keep the presence session Available.
    PresenceActive,
    /// Whether Chaat keeps a presence session at all.
    PresenceSession(bool),
    /// The browser (and profile) to open Microsoft's sign-in page in, so
    /// Teams on the web shares the session; `None` uses the default browser.
    SignInBrowser(Option<crate::calling::browser::BrowserProfile>),
    /// Finds the chat of the meeting with this join link.
    FindMeetingChat(String),
    Edit {
        conversation: ConversationId,
        message: String,
        html: String,
    },
    Delete {
        conversation: ConversationId,
        message: String,
    },
    React {
        conversation: ConversationId,
        message: String,
        emoji: String,
        on: bool,
    },
    MarkRead(String),
    /// Hides (`hide`) or unhides a chat for the person.
    HideChat {
        chat: String,
        hide: bool,
    },
    /// Marks a chat unread for the person.
    MarkChatUnread(String),
    PinMessage {
        chat: String,
        message: String,
    },
    UnpinMessage {
        chat: String,
        pin: String,
    },
    /// Finds the join link of the meeting with this meeting ID.
    FindMeetingById(String),
    /// The push relay to use, or none (docs/realtime.md).
    Relay(Option<String>),
    /// Where a push plugin receives Graph's posts (docs/realtime.md), or none.
    /// A relay set with `Relay` wins.
    PushPlugin(Option<PushRoute>),
    /// A body the push plugin forwarded; dropped when no plugin loop runs.
    PushBody(String),
    /// Stores the relay's access key in the credential store, or removes it
    /// (`None`). Sent to the service backend; answered with `RelayKeySaved`.
    SaveRelayKey(Option<crate::realtime::RelayKey>),
    Presence(Vec<String>),
    Photo(String),
    /// A person's display name, for a system event Graph left it out of.
    UserName(String),
    Media(String),
    Search(String),
    /// Find in one open conversation. Without `remote`, local hits only.
    /// With it (the query has paused), the local hits merged with Microsoft
    /// Search hits for the same conversation, in one event.
    SearchIn {
        conversation: ConversationId,
        query: String,
        remote: bool,
    },
    /// Launch (or relaunch) one plugin of the plugin list. Replaces only the
    /// host with this id.
    StartPlugin {
        id: String,
        path: std::path::PathBuf,
        hello: crate::plugin::protocol::HostMessage,
        /// The file's pin, if it has one (`PluginEntry::sha256`): a file that
        /// does not match is not run.
        expected_sha256: Option<String>,
        /// Names this start. Events from it carry it back, so the interface
        /// can drop what an older run of the same plugin said.
        generation: u64,
    },
    /// Sends a message to one running plugin.
    PluginSend {
        id: String,
        message: crate::plugin::protocol::HostMessage,
    },
    /// Changes what one plugin's restart hello holds, for each part given.
    PluginHello {
        id: String,
        accounts: Option<Vec<crate::plugin::protocol::PluginAccount>>,
        forget: Option<Vec<String>>,
    },
    /// Sends a plugin its option values. The worker adds the values of the
    /// `secrets` options from the credential store; a missing one is left out.
    PluginOptions {
        id: String,
        values: std::collections::BTreeMap<String, serde_json::Value>,
        secrets: Vec<String>,
    },
    /// Stores one secret option's value in the credential store, or removes
    /// it (`None`). Answered with `PluginSecretSaved`.
    SavePluginSecret {
        plugin: String,
        /// The plugin entry's `instance`, echoed in the answer so a late
        /// answer for a removed plugin of the same id can be ignored.
        instance: String,
        option: String,
        value: Option<String>,
    },
    /// Stop one plugin, if it runs. The others keep running.
    StopPlugin {
        id: String,
    },
    /// Open a setup file with its passphrase; the answer is `Event::SetupOpened`.
    OpenSetupFile {
        path: std::path::PathBuf,
        passphrase: String,
        token: u64,
    },
    /// Apply the setup file opened under `token`; the answer is `Event::SetupApplied`.
    ApplySetup {
        token: u64,
    },
    /// Forget the setup file opened under `token`.
    DiscardSetup {
        token: u64,
    },
    /// Write a setup file; the answer is `Event::SetupExported`.
    ExportSetup {
        path: std::path::PathBuf,
        payload: crate::setup_file::SetupPayload,
        include_relay_key: bool,
    },
    Shutdown,
}

/// How a batch of messages relates to what the interface already holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// The newest page: replaces what is shown, keeping older loaded pages
    /// only when they connect.
    Latest,
    /// Changes since the last refresh, merged by id.
    Changes,
    /// An older page, added before what is shown.
    Older,
}

#[derive(Debug)]
pub enum Event {
    Update(crate::updates::Event),
    Pasted {
        conversation: ConversationId,
        range: (usize, usize),
        text: String,
        image: Result<Option<crate::clipboard::InlineImage>, &'static str>,
    },
    Auth(AuthStatus),
    Profile(Box<Profile>),
    /// The scopes the session holds, space-separated.
    Granted(String),
    Chats {
        chats: Vec<Chat>,
        cached: bool,
    },
    Teams {
        teams: Vec<Team>,
        cached: bool,
    },
    Channels {
        team: Option<String>,
        channels: Vec<Channel>,
    },
    Messages {
        conversation: ConversationId,
        messages: Vec<Message>,
        merge: Merge,
        /// Whether older messages exist beyond these.
        more: Option<bool>,
        cached: bool,
    },
    Replies {
        conversation: ConversationId,
        root: String,
        replies: Vec<Message>,
    },
    Sent {
        conversation: ConversationId,
        local_id: String,
        message: Box<Message>,
    },
    SendFailed {
        conversation: ConversationId,
        local_id: String,
        reason: String,
    },
    /// Drafts and unconfirmed messages saved before the last exit, once per
    /// account start, after the cached chats.
    Restored {
        drafts: Vec<(ConversationId, crate::outbox::StoredDraft)>,
        outbox: Vec<crate::outbox::OutboxEntry>,
    },
    Deleted {
        conversation: ConversationId,
        message: String,
    },
    /// Whether channel posts can be read with the granted scopes.
    ChannelAccess(Access),
    Presence(Vec<(String, Presence)>),
    UserName {
        user: String,
        name: Option<String>,
    },
    Photo {
        user: String,
        bytes: Option<Vec<u8>>,
    },
    Media {
        url: String,
        bytes: Option<Vec<u8>>,
    },
    Meetings {
        meetings: Vec<Meeting>,
        cached: bool,
    },
    SearchResults {
        query: String,
        hits: Vec<SearchHit>,
        remote: bool,
    },
    /// Hits of `Command::SearchIn`. Each event holds every hit known so far
    /// (a later event replaces an earlier one), all in `conversation`.
    ConversationHits {
        conversation: ConversationId,
        query: String,
        hits: Vec<SearchHit>,
    },
    /// A fetch started (`true`) or ended.
    Loading(Resource, bool),
    /// The push relay's state.
    Push(crate::realtime::PushStatus),
    /// Whether `SaveRelayKey` worked; `set` says whether a key is now stored.
    RelayKeySaved {
        set: bool,
        ok: bool,
    },
    /// Something a plugin did, by the plugin's id.
    Plugin {
        id: String,
        /// The `generation` of the `StartPlugin` whose host reported this.
        generation: u64,
        event: crate::plugin::event::PluginEvent,
    },
    /// Whether `SavePluginSecret` worked; `set` says whether a value is now
    /// stored.
    PluginSecretSaved {
        plugin: String,
        /// The `instance` of the `SavePluginSecret` this answers.
        instance: String,
        option: String,
        set: bool,
        ok: bool,
    },
    /// Directory search results for `query`.
    People {
        query: String,
        people: Vec<Member>,
    },
    ChatCreated(Box<Chat>),
    /// Everyone in a group chat, as Graph lists them.
    Members {
        chat: String,
        members: Vec<Member>,
    },
    /// The outcome of `AddMembers`: who was added and who was not.
    MembersAdded {
        chat: String,
        added: Vec<String>,
        failed: Vec<String>,
    },
    /// Whether hiding or unhiding a chat worked.
    ChatHidden {
        chat: String,
        hidden: bool,
        ok: bool,
    },
    /// Marking a chat unread failed.
    MarkUnreadFailed(String),
    /// The pinned messages of a chat.
    Pins {
        chat: String,
        pins: Vec<PinnedMessage>,
    },
    /// The join link of a meeting found by its meeting ID (`None`: none).
    MeetingById {
        id: String,
        join_url: Option<String>,
    },
    /// The person left the chat.
    LeftChat(String),
    /// Leaving failed; `admin` when Graph refused for lack of an
    /// administrator-approved permission (403).
    LeaveFailed {
        chat: String,
        admin: bool,
    },
    /// The signed-in person's own presence.
    MyPresence(Presence),
    /// The automatic replies, as Graph has them after a fetch or a save.
    OutOfOffice(crate::model::OutOfOffice),
    /// The account has no Exchange Online mailbox, so no automatic replies.
    OutOfOfficeUnavailable,
    /// Saving the automatic replies failed (the reason went out as an error).
    OutOfOfficeNotSaved,
    /// The chat of a meeting, looked up by its join link (`None`: not found).
    MeetingChat {
        join_url: String,
        chat: Option<Box<Chat>>,
    },
    /// Whether Microsoft could be reached on the last fetch. Sent only when it
    /// changes; a lost connection is shown quietly, not as an error.
    Reachable(bool),
    /// A setup file was opened (the summary) or could not be (a fixed text).
    SetupOpened {
        token: u64,
        result: Result<crate::setup_file::SetupSummary, String>,
    },
    /// Whether the setup file opened under `token` was applied.
    SetupApplied {
        token: u64,
        ok: bool,
    },
    /// A setup file was written (the passphrase) or could not be (a fixed text).
    SetupExported(Result<String, String>),
    /// Something the person should hear about, in plain words.
    Error(String),
}

/// Where Graph posts notifications for a push plugin, and the channel that
/// tells the plugin which account a post is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushRoute {
    /// An https origin without a trailing slash.
    pub url: String,
    pub channel: String,
}

impl PushRoute {
    pub fn new(url: String, channel: String) -> PushRoute {
        PushRoute { url, channel }
    }
}

/// Where the backend signs in and fetches from, and where it keeps secrets.
/// The defaults are Microsoft's endpoints and the OS credential store; tests
/// point them at local mock servers and an in-memory store.
#[derive(Clone)]
pub struct BackendOptions {
    pub authority: String,
    pub graph_base: String,
    pub secrets: std::sync::Arc<dyn crate::secrets::SecretStore>,
    /// Open the system browser for sign-in (off in tests).
    pub open_browser: bool,
    /// How often push renews its subscriptions ([`crate::realtime::RENEW_EVERY`];
    /// shorter in tests).
    pub push_renew_every: std::time::Duration,
    /// How long the WebSocket to the relay may take to open before the
    /// attempt counts as failed and is retried (8 seconds: the relay answers
    /// at once, so a longer wait only delays the retry; shorter in tests).
    pub relay_connect_timeout: std::time::Duration,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            authority: crate::auth::DEFAULT_AUTHORITY.to_owned(),
            graph_base: crate::graph::GRAPH_BASE.to_owned(),
            secrets: crate::secrets::native(),
            open_browser: true,
            push_renew_every: crate::realtime::RENEW_EVERY,
            relay_connect_timeout: std::time::Duration::from_secs(8),
        }
    }
}

pub struct Backend {
    commands: mpsc::UnboundedSender<Command>,
    events: std::sync::mpsc::Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
    detached: bool,
}

impl Backend {
    pub fn spawn(dirs: AppDirs, waker: Waker) -> Self {
        Self::spawn_with(dirs, waker, BackendOptions::default())
    }

    pub fn spawn_with(dirs: AppDirs, waker: Waker, options: BackendOptions) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("chaat-runtime")
            .enable_all()
            .build()
            .expect("unable to start the async runtime");
        let thread = std::thread::Builder::new()
            .name("chaat-backend".into())
            .spawn(move || {
                runtime.block_on(worker::run(dirs, options, event_tx, command_rx, waker));
                runtime.shutdown_timeout(Duration::from_secs(2));
            })
            .expect("unable to start the backend thread");
        Self {
            commands: command_tx,
            events: event_rx,
            thread: Some(thread),
            detached: false,
        }
    }

    /// A backend with no runtime, for the demo and tests. Commands are
    /// dropped; the returned sender delivers events.
    pub fn detached() -> (Self, std::sync::mpsc::Sender<Event>) {
        let (command_tx, _) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        (
            Self {
                commands: command_tx,
                events: event_rx,
                thread: None,
                detached: true,
            },
            event_tx,
        )
    }

    /// Records commands for a test.
    pub fn recording() -> (Self, mpsc::UnboundedReceiver<Command>) {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (_, event_rx) = std::sync::mpsc::channel();
        (
            Self {
                commands: command_tx,
                events: event_rx,
                thread: None,
                detached: false,
            },
            command_rx,
        )
    }

    pub fn is_detached(&self) -> bool {
        self.detached
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Asks the backend to stop without waiting for it, so several can wind
    /// down (delete push subscriptions, end presence sessions) at once.
    pub fn begin_shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown);
    }

    pub fn shutdown(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.shutdown();
    }
}
