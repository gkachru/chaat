//! Application state and the one place it changes.
//!
//! Views in `ui/` read [`App`] and push [`Action`]s; [`App::apply`] runs
//! them after the frame. Backend [`Event`]s are applied at the start of each
//! frame. Nothing here waits on the network.
//!
//! Several Microsoft accounts can be signed in at once. Each [`Account`] has
//! its own backend (runtime thread, Graph session, cache, credentials) and
//! its own view state; [`App`] dereferences to the account on screen, so
//! views draw `app.chats` without knowing there are others. Events from an
//! account that is not on screen are applied with `hidden` set: they update
//! that account, notify, and never touch what the window shows.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::backend::{AuthStatus, Backend, Command, Event, Merge, Waker};
use crate::html;
use crate::model::{
    Access, Action, Availability, Channel, Chat, ChatKind, ConversationId, Delivery, Meeting,
    Member, MentionRef, Message, Presence, Profile, Reaction, Ring, SearchHit, Section,
    SettingsPage, Team, Toast, ToastKind,
};
use crate::paths::AppDirs;
use crate::settings::Settings;
use crate::sync::{Resource, Trigger};
use crate::theme::Palette;

mod calls;
mod history;
mod plugins;
mod push;
mod setup;
mod setup_events;
mod setup_export;
pub use calls::Docked;
pub use history::HistoryLayout;
pub use plugins::{ADD_START_LIMIT, PluginState, PluginSummary, PluginsView, SummaryLevel};
pub use push::{PushPluginState, PushPluginView};

/// Messages and view state of one conversation.
#[derive(Debug, Default)]
pub struct Conversation {
    pub layout: HistoryLayout,
    /// Oldest first.
    pub messages: Vec<Message>,
    /// Older messages exist beyond the loaded ones.
    pub more: bool,
    /// Graph answered at least once (not only the cache).
    pub fresh: bool,
    /// Measured row heights, by message id (virtualized list).
    pub heights: HashMap<String, f32>,
    /// Channel threads shown expanded.
    pub expanded: HashSet<String>,
    /// First visible row and its offset from the viewport top.
    pub history_anchor: Option<(String, f32)>,
    pub restore_history_anchor: bool,
    /// The person's last read time when they opened this chat with unread
    /// messages: the "Last read" divider goes before the first newer
    /// message from someone else.
    pub unread_after: Option<crate::model::Millis>,
    /// The message the divider was last drawn above.
    pub divider_at: Option<String>,
}

impl Conversation {
    /// Moves or removes the "Last read" divider.
    pub fn set_unread_after(&mut self, after: Option<crate::model::Millis>) {
        self.unread_after = after;
    }

    /// Index of the first message after the "Last read" divider. The row
    /// above which the divider sits measures it, so when the divider moves
    /// (a new read time, or older history loaded), the old and new rows are
    /// measured again.
    pub fn place_divider(&mut self, me: &str) -> Option<usize> {
        let index = self.unread_after.and_then(|after| {
            let start = self.messages.partition_point(|m| m.created <= after);
            self.messages[start..]
                .iter()
                .position(|m| !m.system && !m.is_from(me))
                .map(|offset| start + offset)
        });
        let id = index.map(|i| self.messages[i].id.clone());
        if id != self.divider_at {
            for stale in [&self.divider_at, &id].into_iter().flatten() {
                self.heights.remove(stale);
            }
            self.layout.invalidate();
            self.divider_at = id;
        }
        index
    }

    pub fn find(&self, id: &str) -> Option<&Message> {
        self.messages.iter().find(|m| m.id == id).or_else(|| {
            self.messages
                .iter()
                .flat_map(|m| m.replies.iter())
                .find(|m| m.id == id)
        })
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut Message> {
        self.layout.invalidate();
        self.heights.remove(id);
        if let Some(index) = self.messages.iter().position(|m| m.id == id) {
            return self.messages.get_mut(index);
        }
        let root = self
            .messages
            .iter_mut()
            .find(|m| m.replies.iter().any(|r| r.id == id))?;
        self.heights.remove(&root.id);
        root.replies.iter_mut().find(|m| m.id == id)
    }

    /// Merges messages by id, keeping messages still being sent.
    pub fn merge(&mut self, incoming: Vec<Message>, merge: Merge) {
        self.layout.invalidate();
        for message in &incoming {
            self.heights.remove(&message.id);
        }
        match merge {
            Merge::Latest if self.messages.iter().all(|m| m.delivery == Delivery::Sent) => {
                // Keep older loaded messages when the new page connects to
                // them (overlaps by id).
                let oldest_new = incoming.iter().map(|m| m.created).min();
                let overlaps = incoming
                    .iter()
                    .any(|m| self.messages.iter().any(|o| o.id == m.id));
                let mut keep: Vec<Message> = if overlaps {
                    self.messages
                        .drain(..)
                        .filter(|m| oldest_new.is_some_and(|oldest| m.created < oldest))
                        .collect()
                } else {
                    Vec::new()
                };
                keep.extend(incoming);
                self.messages = keep;
            }
            _ => {
                for message in incoming {
                    if let Some(reply_to) = message.reply_to.clone()
                        && message
                            .conversation
                            .as_ref()
                            .is_some_and(ConversationId::is_channel)
                        && let Some(root) = self.messages.iter_mut().find(|m| m.id == reply_to)
                    {
                        match root.replies.iter_mut().find(|r| r.id == message.id) {
                            Some(existing) => *existing = message,
                            None => root.replies.push(message),
                        }
                        continue;
                    }
                    match self.messages.iter_mut().find(|m| m.id == message.id) {
                        Some(existing) => {
                            let replies = std::mem::take(&mut existing.replies);
                            *existing = message;
                            if existing.replies.is_empty() {
                                existing.replies = replies;
                            }
                        }
                        None => self.messages.push(message),
                    }
                }
            }
        }
        self.messages.sort_by(|a, b| {
            // Unsent messages stay at the end.
            (a.delivery != Delivery::Sent, a.created, &a.id).cmp(&(
                b.delivery != Delivery::Sent,
                b.created,
                &b.id,
            ))
        });
        self.messages.dedup_by(|a, b| a.id == b.id);
    }
}

/// What the composer of one conversation holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    /// The chat message being quoted, or the channel post being replied to.
    pub reply_to: Option<String>,
    /// The message being edited.
    pub editing: Option<String>,
    /// People picked with `@`, in order.
    pub mentions: Vec<MentionRef>,
    /// Files to send with the message.
    pub files: Vec<PathBuf>,
    pub images: Vec<crate::clipboard::InlineImage>,
    pub pending_pastes: usize,
    pub paste_cursor: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Overlay {
    Switcher,
    NewChat,
    StatusMessage,
    /// A feature needs a permission the account has not granted yet.
    Consent(&'static str),
    /// A join link as a QR code, to join on a phone.
    JoinOnPhone(String),
    ImagePreview(String),
    /// Add people to a group chat (chat id).
    AddPeople(String),
    /// Leave a group chat (chat id): a confirmation, or an explanation of the
    /// permission it needs.
    LeaveChat(String),
}

/// The Add people dialog's fields.
#[derive(Debug, Default)]
pub struct AddPeople {
    pub query: String,
    pub picked: Vec<Member>,
    pub selected: usize,
    /// The second step, choosing what the new people see of the chat so far.
    pub history_step: bool,
    /// Whether new people see the earlier messages (false: none, true: all).
    pub history: bool,
}

#[derive(Debug, Default)]
pub struct Switcher {
    pub query: String,
    pub selected: usize,
    pub hits: Vec<SearchHit>,
    pub remote_hits: Vec<SearchHit>,
    pub searched: String,
}

/// The new-chat dialog.
#[derive(Debug, Default)]
pub struct NewChat {
    pub query: String,
    pub picked: Vec<Member>,
    pub topic: String,
    pub selected: usize,
}

/// Directory search results, shared by the new-chat dialog and mentions.
#[derive(Debug, Default)]
pub struct People {
    pub query: String,
    pub results: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Image {
    Loading,
    Ready(String),
    Missing,
}

/// A send Graph has not confirmed, kept for Retry.
#[derive(Clone, Debug)]
struct PendingSend {
    html: String,
    reply_to: Option<String>,
    mentions: Vec<MentionRef>,
    files: Vec<PathBuf>,
    images: Vec<crate::clipboard::InlineImage>,
    recipients: Vec<String>,
}

/// Frame timing for the performance overlay.
#[derive(Debug, Default)]
pub struct Perf {
    pub cpu: Vec<f32>,
    pub frames: u64,
    pub started: Option<Instant>,
    pub first_frame: Option<Duration>,
}

impl Perf {
    pub fn record(&mut self, cpu: Option<f32>) {
        self.frames += 1;
        if let Some(cpu) = cpu {
            if self.cpu.len() >= 120 {
                self.cpu.remove(0);
            }
            self.cpu.push(cpu);
        }
    }

    pub fn average_ms(&self) -> f32 {
        if self.cpu.is_empty() {
            return 0.0;
        }
        self.cpu.iter().sum::<f32>() / self.cpu.len() as f32 * 1000.0
    }
}

/// One Microsoft account: its backend and everything Chaat shows for it.
pub struct Account {
    pub backend: Backend,
    /// `oid.tid`, once signed in or restored.
    pub id: Option<String>,
    /// The path part of the address the push plugin forwards this account's
    /// notifications on. Random per run, never stored.
    pub(crate) push_channel: String,
    pub auth: AuthStatus,
    pub profile: Option<Profile>,
    pub granted: String,
    pub channel_access: Access,

    pub chats: Vec<Chat>,
    pub teams: Vec<Team>,
    pub channels: HashMap<String, Vec<Channel>>,
    pub conversations: HashMap<ConversationId, Conversation>,
    pub presence: HashMap<String, Presence>,
    /// The signed-in person's own presence.
    pub my_presence: Option<Presence>,
    pub meetings: Vec<Meeting>,
    pub photos: HashMap<String, Image>,
    /// Display names Graph left out of system events, looked up by id.
    pub names: HashMap<String, String>,
    names_asked: HashSet<String>,
    pub media: HashMap<String, Image>,
    pub loading: HashSet<Resource>,

    pub open: Option<ConversationId>,
    /// People being added to group chats, shown as "Adding…": (chat id, user id).
    pub adding: BTreeSet<(String, String)>,
    /// Chats the person is leaving, kept to bring back if leaving fails.
    pub leaving: HashMap<String, Chat>,
    /// Chats already left, kept out of the list until Graph's chat list
    /// stops returning them (it can lag behind).
    pub left: HashSet<String>,
    pub expanded_teams: HashSet<String>,
    pub selected_meeting: Option<String>,
    pub chat_filter: String,
    /// The chat list shows only unread chats (and the open one).
    pub unread_only: bool,
    /// The chat list shows group chats. With `meetings_only` off too, every
    /// kind shows; with both on, group and meeting chats show.
    pub groups_only: bool,
    /// The chat list shows meeting chats (see `groups_only`).
    pub meetings_only: bool,
    /// The open chat, when it was unread as it opened: it stays in the
    /// Unread list while it is read.
    pub unread_kept: Option<String>,
    pub drafts: HashMap<ConversationId, Draft>,
    pub push: crate::realtime::PushStatus,

    /// The browser profile folder for this account's calls, when each
    /// account has its own (`settings.calls.profile_per_account`).
    pub browser_profile: String,
    /// The expired status, while this account signs in again. The cached
    /// workspace stays on screen meanwhile, and cancelling returns to it.
    pub reauth: Option<AuthStatus>,
    /// The last fetch could not reach Microsoft. Shown as a quiet marker in
    /// the sidebar until a fetch gets through.
    pub unreachable: bool,
    /// The automatic replies (Out of Office), once fetched.
    pub out_of_office: Option<crate::model::OutOfOffice>,
    /// Saving the automatic replies is under way.
    pub out_of_office_saving: bool,
    /// The account has no Exchange Online mailbox, so no automatic replies.
    pub no_mailbox: bool,

    pending_sends: HashMap<String, PendingSend>,
    next_local: u64,
    reminded: HashSet<String>,
    recent_conversations: VecDeque<ConversationId>,
}

impl Account {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend,
            id: None,
            push_channel: crate::auth::random_token(32),
            auth: AuthStatus::Restoring,
            profile: None,
            granted: String::new(),
            channel_access: Access::Unknown,
            chats: Vec::new(),
            teams: Vec::new(),
            channels: HashMap::new(),
            conversations: HashMap::new(),
            presence: HashMap::new(),
            my_presence: None,
            meetings: Vec::new(),
            photos: HashMap::new(),
            names: HashMap::new(),
            names_asked: HashSet::new(),
            media: HashMap::new(),
            loading: HashSet::new(),
            open: None,
            adding: BTreeSet::new(),
            leaving: HashMap::new(),
            left: HashSet::new(),
            expanded_teams: HashSet::new(),
            selected_meeting: None,
            chat_filter: String::new(),
            unread_only: false,
            groups_only: false,
            meetings_only: false,
            unread_kept: None,
            drafts: HashMap::new(),
            push: crate::realtime::PushStatus::Off,
            browser_profile: format!("Chaat {}", crate::auth::random_token(4)),
            pending_sends: HashMap::new(),
            next_local: 0,
            reminded: HashSet::new(),
            recent_conversations: VecDeque::new(),
            reauth: None,
            unreachable: false,
            out_of_office: None,
            out_of_office_saving: false,
            no_mailbox: false,
        }
    }

    /// The people of a group chat: the signed-in person first, then the
    /// others by name. Empty for a chat Chaat does not know.
    pub fn group_members(&self, chat: &str) -> Vec<Member> {
        let Some(chat) = self.chats.iter().find(|c| c.id == chat) else {
            return Vec::new();
        };
        let me = self.me();
        let mut members = chat.members.clone();
        members.sort_by_key(|m| (m.user_id != me, m.display_name.to_lowercase()));
        members
    }

    /// The signed-in person's id (Graph user id).
    pub fn me(&self) -> &str {
        if let Some(profile) = &self.profile {
            return &profile.id;
        }
        match &self.auth {
            AuthStatus::SignedIn { account, .. } => account.split('.').next().unwrap_or_default(),
            _ => self
                .id
                .as_deref()
                .and_then(|id| id.split('.').next())
                .unwrap_or_default(),
        }
    }

    /// The account's name for menus and notifications.
    pub fn label(&self) -> String {
        if let Some(profile) = &self.profile
            && !profile.display_name.is_empty()
        {
            return profile.display_name.clone();
        }
        match &self.auth {
            AuthStatus::SignedIn { name, username, .. } if !name.is_empty() => {
                let _ = username;
                name.clone()
            }
            AuthStatus::SignedIn { username, .. } | AuthStatus::Expired { username, .. } => {
                username.clone()
            }
            _ => "New account".into(),
        }
    }

    /// The account's sign-in address.
    pub fn username(&self) -> String {
        match &self.auth {
            AuthStatus::SignedIn { username, .. } | AuthStatus::Expired { username, .. } => {
                username.clone()
            }
            _ => self
                .profile
                .as_ref()
                .map(|p| p.user_principal_name.clone())
                .unwrap_or_default(),
        }
    }

    pub fn is_signed_in_here(&self) -> bool {
        matches!(self.auth, AuthStatus::SignedIn { .. })
    }

    pub fn chat(&self, id: &str) -> Option<&Chat> {
        self.chats.iter().find(|chat| chat.id == id)
    }

    pub fn channel(&self, team: &str, channel: &str) -> Option<&Channel> {
        self.channels.get(team)?.iter().find(|c| c.id == channel)
    }

    pub fn team(&self, team: &str) -> Option<&Team> {
        self.teams.iter().find(|t| t.id == team)
    }

    pub fn conversation(&self, id: &ConversationId) -> Option<&Conversation> {
        self.conversations.get(id)
    }

    pub fn unread_chats(&self) -> usize {
        let me = self.me();
        self.chats
            .iter()
            .filter(|chat| !chat.hidden && chat.is_unread(me))
            .count()
    }

    /// A conversation's title for headers, notifications and search.
    pub fn title_of(&self, conversation: &ConversationId) -> String {
        match conversation {
            ConversationId::Chat(id) => self
                .chat(id)
                .map(|chat| chat.title(self.me()))
                .unwrap_or_else(|| "Chat".into()),
            ConversationId::Channel { team, channel } => {
                let team_name = self.team(team).map(|t| t.name.as_str()).unwrap_or("Team");
                let channel_name = self
                    .channel(team, channel)
                    .map(|c| c.name.as_str())
                    .unwrap_or("Channel");
                format!("{team_name} › {channel_name}")
            }
        }
    }

    /// A person's name by id, for system events: "You", someone from the
    /// account's chats, or a name looked up before.
    pub fn person_name(&self, id: &str) -> Option<String> {
        if id == self.me() {
            return Some("You".into());
        }
        self.chats
            .iter()
            .flat_map(|chat| chat.members.iter())
            .find(|m| m.user_id == id && !m.display_name.is_empty())
            .map(|m| m.display_name.clone())
            .or_else(|| self.names.get(id).cloned())
    }

    /// Whether a system event's person is still unnamed and not yet looked
    /// up, so views ask for each name once.
    pub fn needs_name(&self, id: &str) -> bool {
        !self.names_asked.contains(id) && self.person_name(id).is_none()
    }

    /// Everyone in the account's chats except the person, by name: the
    /// local directory for mentions and new chats.
    pub fn directory(&self) -> Vec<Member> {
        let me = self.me();
        let mut people: Vec<Member> = Vec::new();
        for member in self.chats.iter().flat_map(|chat| chat.members.iter()) {
            if member.user_id != me
                && !member.display_name.is_empty()
                && !people.iter().any(|p| p.user_id == member.user_id)
            {
                people.push(member.clone());
            }
        }
        people.sort_by_key(|p| p.display_name.to_lowercase());
        people
    }

    fn clear_data(&mut self) {
        self.profile = None;
        self.granted.clear();
        self.chats.clear();
        self.teams.clear();
        self.channels.clear();
        self.conversations.clear();
        self.recent_conversations.clear();
        self.presence.clear();
        self.my_presence = None;
        self.meetings.clear();
        self.photos.clear();
        self.media.clear();
        self.drafts.clear();
        self.open = None;
    }
}

pub use setup::{ONBOARDING_ENV, SetupFlow, SetupStage, is_setup_file, onboarding_forced_by_env};
pub use setup_export::ExportForm;

pub struct App {
    /// The setup file flow (onboarding and settings).
    pub setup: setup::SetupFlow,
    /// `CHAAT_ONBOARDING=1`: show the welcome screen even when configured,
    /// exactly as on a first run, until a file is applied or the form saved.
    pub onboarding_forced: bool,
    /// The setup file export form.
    pub export: setup_export::ExportForm,
    /// Process-wide work survives account switching and sign-out.
    pub service: Backend,
    pub update: crate::updates::Status,
    pub update_receipt: Option<fastframe_update::Receipt>,
    pub dirs: AppDirs,
    pub settings: Settings,
    settings_dirty: bool,
    pub waker: Waker,
    pub demo: bool,

    pub accounts: Vec<Account>,
    pub active: usize,
    /// Events being applied belong to an account that is not on screen.
    hidden: bool,
    /// The account shown before "Add account", to go back to.
    adding_from: Option<usize>,
    /// Account slots to drop after this round of events (duplicates).
    retire: Vec<usize>,

    pub section: Section,
    /// The settings page in place of the sidebar and content, if open.
    pub settings_page: Option<SettingsPage>,
    /// The settings page shown last, to open again with ⌘,.
    last_settings_page: SettingsPage,
    pub overlay: Option<Overlay>,
    pub switcher: Switcher,
    pub new_chat: NewChat,
    pub add_people: AddPeople,
    pub people: People,
    /// A message to scroll to once its conversation is drawn.
    pub jump_to: Option<String>,
    pub toasts: Vec<Toast>,
    /// Ringing calls the call-alerts plugin reported, newest last.
    pub rings: Vec<Ring>,
    /// Which rings already raised the window, and when it last was.
    ring_raises: calls::RingRaises,
    /// The plugins of the plugin list at run time, by id. Process-wide, like
    /// the plugins themselves.
    pub plugins: std::collections::BTreeMap<String, PluginState>,
    /// The plugins started, by id, and the path each was started from, so a
    /// changed path restarts that plugin only.
    plugins_started: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// The generation each plugin was last started (or stopped) with, and the
    /// last one handed out. Events carry the generation of the start that
    /// produced them; a stopped plugin's entry names a generation no host has.
    plugin_generations: std::collections::BTreeMap<String, u64>,
    plugin_generation: u64,
    /// The push plugin's route; see `app/push.rs`.
    push_plugin: push::PushRouting,
    /// When each plugin last raised a toast, so a chatty one is limited.
    plugin_toasts: std::collections::BTreeMap<String, std::time::Instant>,
    /// What is wrong with the value last entered for a plugin option, by
    /// (plugin id, option id); shown under the field until a valid value is
    /// entered.
    pub plugin_option_errors: std::collections::BTreeMap<(String, String), String>,
    /// Which view of Settings › Plugins shows. Interface state, never saved.
    pub plugins_view: PluginsView,
    /// Whether the plugins list is in reorder mode.
    pub reordering: bool,
    /// When each plugin was last asked to start, for the add flow's wait.
    plugin_started_at: std::collections::BTreeMap<String, std::time::Instant>,
    next_toast: u64,
    pub actions: Vec<Action>,

    /// Forms.
    pub form_client_id: String,
    pub form_tenant: String,
    pub form_relay: String,
    /// The relay access key being typed. Cleared once saved; never written
    /// to the settings file.
    pub form_relay_key: String,
    pub form_status: String,
    /// The Out of Office page's fields, for the account on screen.
    pub form_out_of_office: crate::model::OutOfOfficeForm,

    pub palette: Palette,
    applied_palette: Option<Palette>,
    pub window_focused: bool,
    pub window_hidden: bool,
    pub quit_requested: bool,
    pub wants_show: bool,
    pub perf: Perf,
    pub tray: Option<fastframe_tray::Tray>,
    pub requests: Option<std::sync::mpsc::Receiver<String>>,
    pub focus_composer: bool,
    /// Frames left in which the open history scrolls to its newest message.
    pub scroll_to_end: u8,
    /// When the person last touched the window (egui time), for presence.
    last_input: Option<f64>,
    /// Opens the account menu on the next frame (screenshots, tests).
    pub open_account_menu: bool,

    /// The call or meeting the call bar follows.
    pub call: Option<crate::calling::session::CallSession>,
    /// The account whose call bar the person closed while presence still
    /// said they were in a call. No new bar shows for it until presence
    /// says the call is over.
    call_dismissed: Option<Option<String>>,
    /// Chaat's window before it moved beside a call window.
    pub docked: Option<Docked>,
    /// Edge and Chrome, found on first use.
    browsers: Option<Vec<crate::calling::browser::Browser>>,
}

impl Drop for App {
    fn drop(&mut self) {
        if let crate::updates::Status::Ready(prepared) = std::mem::take(&mut self.update) {
            self.service.send(Command::DiscardUpdate(prepared));
        }
    }
}

impl Deref for App {
    type Target = Account;

    fn deref(&self) -> &Account {
        &self.accounts[self.active]
    }
}

impl DerefMut for App {
    fn deref_mut(&mut self) -> &mut Account {
        &mut self.accounts[self.active]
    }
}

impl App {
    pub fn new(dirs: AppDirs, settings: Settings, waker: Waker, demo: bool) -> Self {
        let mut app = Self::empty(dirs, settings, waker, demo);
        if demo {
            app.accounts.push(Account::new(Backend::detached().0));
            crate::demo::populate(&mut app);
        } else {
            let ids = app.settings.accounts.clone();
            if ids.is_empty() {
                app.add_slot(None);
            }
            for id in &ids {
                app.add_slot(Some(id.clone()));
            }
            app.active = app
                .settings
                .account
                .as_ref()
                .and_then(|active| ids.iter().position(|id| id == active))
                .unwrap_or(0);
        }
        app
    }

    /// Connects the service backend (plugins, updates, secrets) and starts
    /// the plugins that are turned on.
    pub fn attach_service(&mut self, service: Backend) {
        self.service = service;
        self.sync_plugins();
    }

    /// An app with one account slot on `backend` (tests).
    pub fn with_backend(
        dirs: AppDirs,
        settings: Settings,
        waker: Waker,
        backend: Backend,
        demo: bool,
    ) -> Self {
        let mut app = Self::empty(dirs, settings, waker, demo);
        app.accounts.push(Account::new(backend));
        app
    }

    fn empty(dirs: AppDirs, settings: Settings, waker: Waker, demo: bool) -> Self {
        let palette = crate::theme::resolve(&settings.theme, true);
        Self {
            setup: Default::default(),
            onboarding_forced: false,
            export: Default::default(),
            form_client_id: settings.client_id.clone(),
            service: Backend::detached().0,
            update: crate::updates::Status::default(),
            update_receipt: None,
            form_tenant: settings.tenant.clone(),
            form_relay: settings.relay_url.clone().unwrap_or_default(),
            form_relay_key: String::new(),
            form_status: String::new(),
            form_out_of_office: Default::default(),
            dirs,
            settings,
            settings_dirty: false,
            waker,
            demo,
            accounts: Vec::new(),
            active: 0,
            hidden: false,
            adding_from: None,
            retire: Vec::new(),
            section: Section::Chats,
            settings_page: None,
            last_settings_page: SettingsPage::default(),
            overlay: None,
            switcher: Switcher::default(),
            new_chat: NewChat::default(),
            add_people: AddPeople::default(),
            people: People::default(),
            jump_to: None,
            toasts: Vec::new(),
            rings: Vec::new(),
            ring_raises: calls::RingRaises::default(),
            plugins: Default::default(),
            plugins_started: Default::default(),
            plugin_generations: Default::default(),
            push_plugin: Default::default(),
            plugin_generation: 0,
            plugin_toasts: Default::default(),
            plugin_option_errors: Default::default(),
            plugins_view: PluginsView::default(),
            reordering: false,
            plugin_started_at: Default::default(),
            next_toast: 0,
            actions: Vec::new(),
            palette,
            applied_palette: None,
            window_focused: true,
            window_hidden: false,
            quit_requested: false,
            wants_show: false,
            perf: Perf::default(),
            tray: None,
            requests: None,
            focus_composer: false,
            scroll_to_end: 4,
            last_input: None,
            open_account_menu: false,
            call: None,
            call_dismissed: None,
            docked: None,
            browsers: None,
        }
    }

    /// Starts a backend for an account (or a new sign-in when `None`) and
    /// adds its slot.
    fn add_slot(&mut self, id: Option<String>) -> usize {
        let backend = Backend::spawn(self.dirs.clone(), self.waker.clone());
        let mut account = Account::new(backend);
        if let Some(profile) = id
            .as_ref()
            .and_then(|id| self.settings.calls.profiles.get(id))
        {
            account.browser_profile = profile.clone();
        }
        account.id = id;
        self.accounts.push(account);
        let index = self.accounts.len() - 1;
        // The plugin learns the new channel before the new backend uses it.
        self.sync_push_route(false);
        self.start_account(index);
        self.sync_call_browser();
        index
    }

    /// Tells every account's backend which relay to use; each reads the
    /// access key from the credential store when it connects.
    fn send_relay(&self) {
        if self.demo {
            return;
        }
        for account in &self.accounts {
            account
                .backend
                .send(Command::Relay(self.settings.relay_url.clone()));
        }
    }

    fn start_account(&mut self, index: usize) {
        let configured = self.settings.has_app_registration();
        let start = Command::Start {
            client_id: self.settings.client_id.clone(),
            tenant: self.settings.tenant.clone(),
            account: self.accounts[index].id.clone(),
        };
        let relay = Command::Relay(self.settings.relay_url.clone());
        let session = Command::PresenceSession(self.settings.presence_session);
        let route = self.push_route_for(index);
        let account = &mut self.accounts[index];
        account.auth = if configured {
            AuthStatus::Restoring
        } else {
            AuthStatus::NotConfigured
        };
        account.backend.send(start);
        account.backend.send(relay);
        if let Some(route) = route {
            account.backend.send(route);
        }
        account.backend.send(session);
    }

    /// Stops an account's backend without blocking the window: it finishes
    /// what it was doing (a sign-out, say) on its own thread.
    fn retire_slot(&mut self, index: usize) {
        if index >= self.accounts.len() {
            return;
        }
        let account = self.accounts.remove(index);
        let _ = std::thread::Builder::new()
            .name("chaat-retire".into())
            .spawn(move || drop(account));
        // The plugin stops listening on the removed account's channel.
        self.sync_push_route(false);
        if self.active >= self.accounts.len() {
            self.active = self.accounts.len().saturating_sub(1);
        } else if self.active > index {
            self.active -= 1;
        }
    }

    pub fn is_signed_in(&self) -> bool {
        self.demo || matches!(self.auth, AuthStatus::SignedIn { .. })
    }

    /// The account the person was in before "Add account", while adding.
    pub fn adding_from(&self) -> Option<&Account> {
        self.adding_from.and_then(|index| self.accounts.get(index))
    }

    /// Whether the main interface (rather than the sign-in screen) shows.
    pub fn shows_workspace(&self) -> bool {
        if self.onboarding_forced {
            return false;
        }
        self.is_signed_in()
            || ((self.reauth.is_some()
                || matches!(
                    self.auth,
                    AuthStatus::Restoring | AuthStatus::Expired { .. }
                ))
                && !self.chats.is_empty())
    }

    /// Unread chats in accounts other than the one on screen.
    pub fn other_unread(&self) -> usize {
        self.accounts
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.active)
            .map(|(_, account)| account.unread_chats())
            .sum()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.demo || crate::auth::has_scope(&self.granted, scope)
    }

    /// Whether the account holds `scope`; otherwise asks for consent.
    fn ensure_scope(&mut self, scope: &'static str) -> bool {
        if self.has_scope(scope) {
            return true;
        }
        self.overlay = Some(Overlay::Consent(scope));
        false
    }

    /// The Out of Office page opened, or its account changed: the fields
    /// show what Chaat knows, and Graph is asked when the scope is granted.
    fn show_out_of_office(&mut self) {
        self.form_out_of_office = crate::model::OutOfOfficeForm::new(
            &self.out_of_office.clone().unwrap_or_default(),
            crate::time::now(),
        );
        if self.has_scope(crate::auth::MAILBOX_SETTINGS_SCOPE) {
            self.fetch(Resource::OutOfOffice, Trigger::Opened);
        }
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let mut text = text.into();
        if self.hidden && self.accounts.len() > 1 {
            text = format!("{}: {text}", self.label());
        }
        if self.toasts.iter().any(|t| t.text == text) {
            return;
        }
        self.next_toast += 1;
        self.toasts.push(Toast {
            id: self.next_toast,
            kind,
            text,
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    fn fetch(&self, resource: Resource, trigger: Trigger) {
        if !self.demo {
            self.backend.send(Command::Fetch { resource, trigger });
        }
    }

    /// Asks for whatever the current view shows.
    fn fetch_view(&self, trigger: Trigger) {
        match self.section {
            Section::Chats => self.fetch(Resource::Chats, trigger),
            Section::Teams => self.fetch(Resource::Teams, trigger),
            Section::Calendar => self.fetch(Resource::Calendar, trigger),
            Section::Calls => {}
        }
        if let Some(open) = &self.open {
            self.fetch(Resource::Messages(open.clone()), trigger);
        }
    }

    pub fn mark_settings_dirty(&mut self) {
        self.settings_dirty = true;
    }

    pub fn save_settings(&mut self) {
        if self.demo || !self.settings_dirty {
            return;
        }
        self.settings_dirty = false;
        if let Err(error) = self.settings.save(&self.dirs.settings_file()) {
            log::warn!("could not save settings: {error}");
        }
    }

    /// Records which accounts exist and which one shows, for the next start.
    fn remember_accounts(&mut self) {
        let ids: Vec<String> = self.accounts.iter().filter_map(|a| a.id.clone()).collect();
        let active = self.id.clone();
        if self.settings.accounts != ids || self.settings.account != active {
            self.settings.accounts = ids;
            self.settings.account = active;
            self.mark_settings_dirty();
        }
        self.sync_plugin_accounts();
    }

    // -- Frame -----------------------------------------------------------------

    /// Work every frame does, with or without a window: backend events of
    /// every account, tray clicks, reminders.
    pub fn background(&mut self, ctx: &egui::Context) {
        for event in self.service.poll() {
            self.handle(event, ctx);
        }
        if !self.demo
            && !self.service.is_detached()
            && self.settings.automatic_updates
            && !self.update.busy()
            && !matches!(self.update, crate::updates::Status::Ready(_))
        {
            let now = crate::time::now();
            if crate::updates::due(self.settings.last_update_check, now) {
                self.check_update(true);
            }
            let remaining = (86_400_000 - now.saturating_sub(self.settings.last_update_check))
                .clamp(1000, 86_400_000);
            ctx.request_repaint_after(Duration::from_millis(remaining as u64));
        }
        let shown = self.active;
        for index in 0..self.accounts.len() {
            let events = self.accounts[index].backend.poll();
            if events.is_empty() {
                continue;
            }
            log::debug!(
                "account {index}: applying {} backend events (window focused: {})",
                events.len(),
                ctx.input(|i| i.focused)
            );
            self.active = index;
            self.hidden = index != shown;
            for event in events {
                self.handle(event, ctx);
            }
        }
        self.active = shown.min(self.accounts.len().saturating_sub(1));
        self.hidden = false;
        self.retire_duplicates();
        self.handle_tray();
        self.handle_requests();
        self.remind(ctx);
        self.apply_actions(ctx);
        self.save_settings();
    }

    fn retire_duplicates(&mut self) {
        let mut retire = std::mem::take(&mut self.retire);
        retire.sort_unstable();
        retire.dedup();
        for index in retire.into_iter().rev() {
            self.retire_slot(index);
        }
    }

    /// Tracks focus and input; coming back to the window refreshes what it
    /// shows, and using it keeps the presence session Available.
    pub fn observe_window(&mut self, ctx: &egui::Context) {
        // Draft and failed-send sources are authoritative even after the
        // window's bounded display cache has evicted their compressed bytes.
        if let Some(draft) = self.open.as_ref().and_then(|id| self.drafts.get(id)) {
            for image in &draft.images {
                if !crate::images::contains(ctx, &image.uri()) {
                    crate::images::include(ctx, image.uri(), image.png.clone());
                }
            }
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.window_focused {
            self.fetch_view(Trigger::Focused);
            self.fetch(Resource::MyPresence, Trigger::Focused);
            if let Some(ConversationId::Chat(chat)) = self.open.clone() {
                // Messages that arrived while away get a "Last read" divider.
                let me = self.me().to_owned();
                let after = self
                    .chat(&chat)
                    .filter(|c| c.is_unread(&me))
                    .and_then(|c| c.last_read);
                if after.is_some()
                    && let Some(entry) = self
                        .conversations
                        .get_mut(&ConversationId::Chat(chat.clone()))
                {
                    entry.set_unread_after(after);
                }
                self.mark_read(&chat);
            }
        }
        self.window_focused = focused;
        let active_input = ctx.input(|i| {
            i.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                )
            })
        });
        if focused && active_input {
            let now = ctx.input(|i| i.time);
            // Presence needs a refresh only every few minutes.
            if self.last_input.is_none_or(|last| now - last > 60.0) && !self.demo {
                for account in &self.accounts {
                    account.backend.send(Command::PresenceActive);
                }
            }
            self.last_input = Some(now);
        }
        let system_dark = ctx.input(|i| i.raw.system_theme) != Some(egui::Theme::Light);
        self.palette = crate::theme::resolve(&self.settings.theme, system_dark);
        if self.applied_palette != Some(self.palette) {
            crate::theme::apply(ctx, &self.palette);
            self.applied_palette = Some(self.palette);
        }
        let zoom = self.settings.zoom;
        if (ctx.zoom_factor() - zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(zoom);
        }
        self.tick_call(ctx);
    }

    fn handle_tray(&mut self) {
        let Some(tray) = &self.tray else { return };
        for event in tray.events() {
            match event {
                fastframe_tray::Event::Toggle
                | fastframe_tray::Event::Show
                | fastframe_tray::Event::Menu("show") => {
                    self.wants_show = true;
                }
                fastframe_tray::Event::Menu("quit") => self.quit_requested = true,
                fastframe_tray::Event::Menu(_) => {}
            }
        }
    }

    fn handle_requests(&mut self) {
        let Some(requests) = &self.requests else {
            return;
        };
        for request in requests.try_iter().collect::<Vec<_>>() {
            if request == "show" {
                self.wants_show = true;
            }
        }
    }

    /// Meeting reminders from the cached calendars: no network involved.
    fn remind(&mut self, ctx: &egui::Context) {
        if !self.settings.notifications {
            return;
        }
        let now = crate::time::now();
        let lead = 5 * 60 * 1000;
        let mut next: Option<i64> = None;
        let demo = self.demo;
        for account in &mut self.accounts {
            let mut due = Vec::new();
            for meeting in &account.meetings {
                if meeting.cancelled || meeting.all_day || account.reminded.contains(&meeting.id) {
                    continue;
                }
                let at = meeting.start - lead;
                if at <= now && meeting.start > now - 60_000 {
                    due.push(meeting.clone());
                } else if at > now {
                    next = Some(next.map_or(at, |n| n.min(at)));
                }
            }
            for meeting in due {
                account.reminded.insert(meeting.id.clone());
                if !demo {
                    crate::notify::show(
                        &meeting.subject,
                        &format!("Starts {}", crate::time::until_label(meeting.start, now)),
                    );
                }
            }
        }
        if let Some(next) = next {
            ctx.request_repaint_after(Duration::from_millis(
                (next - now).clamp(1_000, 3_600_000) as u64
            ));
        }
    }

    // -- Events ------------------------------------------------------------------

    /// Applies one event to the account at `self.active`.
    pub fn handle(&mut self, event: Event, ctx: &egui::Context) {
        match event {
            Event::Update(event) => match event {
                crate::updates::Event::Status(status) => {
                    if matches!(status, crate::updates::Status::Ready(_)) {
                        self.toast(
                            ToastKind::Info,
                            "An update is downloaded and ready to install when you restart.",
                        );
                    }
                    self.update = status;
                }
                crate::updates::Event::Quit => self.quit_requested = true,
            },
            Event::Pasted {
                conversation,
                range,
                text,
                image,
            } => {
                let draft = self.drafts.entry(conversation.clone()).or_default();
                draft.pending_pastes = draft.pending_pastes.saturating_sub(1);
                match image {
                    Ok(Some(image)) if draft.editing.is_none() => {
                        if draft.images.iter().map(|i| i.png.len()).sum::<usize>() + image.png.len()
                            > crate::clipboard::MAX_BYTES
                        {
                            self.toast(ToastKind::Error, "The pasted images are too large for one message. Send them separately.");
                        } else {
                            crate::images::include(ctx, image.uri(), image.png.clone());
                            draft.images.push(image);
                        }
                    }
                    Ok(Some(_)) => self.toast(
                        ToastKind::Info,
                        "Finish editing before pasting an image into a new message.",
                    ),
                    result => {
                        if !text.is_empty() {
                            let chars: Vec<char> = draft.text.chars().collect();
                            let start = range.0.min(chars.len());
                            let end = range.1.min(chars.len()).max(start);
                            draft.text = format!(
                                "{}{}{}",
                                chars[..start].iter().collect::<String>(),
                                text,
                                chars[end..].iter().collect::<String>()
                            );
                            draft.paste_cursor = Some(start + text.chars().count());
                        } else if let Err(error) = result {
                            self.toast(ToastKind::Error, error);
                        }
                    }
                }
            }
            Event::Auth(status) => self.on_auth(status),
            Event::Profile(profile) => self.profile = Some(*profile),
            event @ (Event::SetupOpened { .. }
            | Event::SetupApplied { .. }
            | Event::SetupExported(_)) => self.on_setup_event(event),
            Event::Granted(granted) => {
                let had = self.has_scope(crate::auth::MAILBOX_SETTINGS_SCOPE);
                self.granted = granted;
                if !self.hidden
                    && !had
                    && self.settings_page == Some(SettingsPage::OutOfOffice)
                    && self.has_scope(crate::auth::MAILBOX_SETTINGS_SCOPE)
                {
                    self.fetch(Resource::OutOfOffice, Trigger::Opened);
                }
            }
            Event::ChannelAccess(access) => self.channel_access = access,
            Event::Chats { chats, cached } => self.on_chats(chats, cached),
            Event::Teams { teams, .. } => {
                if self.expanded_teams.is_empty()
                    && let Some(first) = teams.first()
                {
                    let first = first.id.clone();
                    self.expanded_teams.insert(first);
                }
                self.teams = teams;
            }
            Event::Channels { team, channels } => match team {
                Some(team) => {
                    self.channels.insert(team, channels);
                }
                None => {
                    self.channels.clear();
                    for channel in channels {
                        self.channels
                            .entry(channel.team_id.clone())
                            .or_default()
                            .push(channel);
                    }
                }
            },
            Event::Messages {
                conversation,
                messages,
                merge,
                more,
                cached,
            } => self.on_messages(conversation, messages, merge, more, cached),
            Event::Replies {
                conversation,
                root,
                replies,
            } => {
                let entry = self.conversations.entry(conversation).or_default();
                entry.layout.invalidate();
                entry.heights.remove(&root);
                if let Some(root) = entry.messages.iter_mut().find(|m| m.id == root) {
                    root.replies = replies;
                }
            }
            Event::Sent {
                conversation,
                local_id,
                message,
            } => self.on_sent(conversation, local_id, *message),
            Event::SendFailed {
                conversation,
                local_id,
                reason,
            } => {
                if let Some(entry) = self.conversations.get_mut(&conversation)
                    && let Some(message) = entry.find_mut(&local_id)
                {
                    message.delivery = Delivery::Failed;
                }
                self.toast(ToastKind::Error, format!("Not sent: {reason}"));
            }
            Event::Deleted {
                conversation,
                message,
            } => {
                if let Some(entry) = self.conversations.get_mut(&conversation)
                    && let Some(message) = entry.find_mut(&message)
                {
                    message.deleted = true;
                    message.body = html::RichText::default();
                }
            }
            Event::Presence(list) => self.presence.extend(list),
            Event::MyPresence(presence) => {
                let me = self.me().to_owned();
                if !me.is_empty() {
                    self.presence.insert(me, presence.clone());
                }
                self.observe_call(presence.activity.as_deref());
                self.my_presence = Some(presence);
                self.sync_plugin_presence();
            }
            Event::OutOfOffice(value) => {
                // The fields follow Graph unless the person changed them.
                let follow = self.out_of_office.as_ref().is_none_or(|old| {
                    !self.form_out_of_office.changed_from(old)
                        || !self.form_out_of_office.changed_from(&value)
                });
                if follow && !self.hidden {
                    self.form_out_of_office =
                        crate::model::OutOfOfficeForm::new(&value, crate::time::now());
                }
                self.out_of_office = Some(value);
                self.out_of_office_saving = false;
                self.no_mailbox = false;
            }
            Event::OutOfOfficeUnavailable => {
                self.no_mailbox = true;
                self.out_of_office_saving = false;
            }
            Event::OutOfOfficeNotSaved => self.out_of_office_saving = false,
            Event::MeetingChat { join_url, chat } => self.on_meeting_chat(join_url, chat),
            Event::UserName { user, name } => {
                if let Some(name) = name {
                    self.names.insert(user, name);
                }
            }
            Event::Photo { user, bytes } => {
                let state = match bytes
                    .and_then(|bytes| register_image(ctx, &format!("photo-{user}"), bytes))
                {
                    Some(uri) => Image::Ready(uri),
                    None => Image::Missing,
                };
                self.photos.insert(user, state);
            }
            Event::Media { url, bytes } => {
                let key = format!("media-{:016x}", fnv(&url));
                let state = match bytes.and_then(|bytes| register_image(ctx, &key, bytes)) {
                    Some(uri) => Image::Ready(uri),
                    None => Image::Missing,
                };
                self.media.insert(url, state);
            }
            Event::Meetings { meetings, .. } => self.meetings = meetings,
            Event::SearchResults {
                query,
                hits,
                remote,
            } => {
                if !self.hidden && query == self.switcher.searched {
                    if remote {
                        self.switcher.remote_hits = hits;
                    } else {
                        self.switcher.hits = hits;
                    }
                }
            }
            Event::People { query, people } => {
                if !self.hidden && query == self.people.query {
                    self.people.results = people;
                }
            }
            Event::Members { chat, members } => {
                let pending: Vec<(String, String)> = self
                    .adding
                    .iter()
                    .filter(|(c, _)| *c == chat)
                    .cloned()
                    .collect();
                if let Some(found) = self.chats.iter_mut().find(|c| c.id == chat) {
                    // People still being added stay listed until Graph says.
                    let waiting: Vec<Member> = found
                        .members
                        .iter()
                        .filter(|m| {
                            pending.iter().any(|(_, user)| *user == m.user_id)
                                && !members.iter().any(|n| n.user_id == m.user_id)
                        })
                        .cloned()
                        .collect();
                    found.members = members;
                    found.members.extend(waiting);
                }
            }
            Event::MembersAdded {
                chat,
                added,
                failed,
            } => {
                for user in added.iter().chain(failed.iter()) {
                    self.adding.remove(&(chat.clone(), user.clone()));
                }
                if let Some(found) = self.chats.iter_mut().find(|c| c.id == chat) {
                    found.members.retain(|m| !failed.contains(&m.user_id));
                }
                if !failed.is_empty() {
                    let noun = if failed.len() == 1 {
                        "person"
                    } else {
                        "people"
                    };
                    self.toast(
                        ToastKind::Error,
                        format!("Could not add {} {noun}", failed.len()),
                    );
                }
            }
            Event::LeftChat(chat) => {
                self.leaving.remove(&chat);
                self.chats.retain(|c| c.id != chat);
                self.left.insert(chat);
            }
            Event::LeaveFailed { chat, admin } => {
                if let Some(found) = self.leaving.remove(&chat) {
                    self.chats.retain(|c| c.id != chat);
                    self.chats.insert(0, found);
                    self.chats.sort_by_key(Chat::list_order);
                }
                let text = if admin {
                    "Could not leave the chat. An administrator must approve leaving chats; try Open in Teams."
                } else {
                    "Could not leave the chat"
                };
                self.toast(ToastKind::Error, text);
            }
            Event::ChatCreated(chat) => {
                let chat = *chat;
                let id = chat.id.clone();
                self.chats.retain(|c| c.id != id);
                self.chats.insert(0, chat);
                if !self.hidden {
                    self.open_conversation(ConversationId::Chat(id));
                }
            }
            Event::Loading(resource, on) => {
                if on {
                    self.loading.insert(resource);
                } else {
                    self.loading.remove(&resource);
                }
            }
            Event::Reachable(reachable) => self.unreachable = !reachable,
            Event::Error(text) => self.toast(ToastKind::Error, text),
            Event::Push(status) => self.push = status,
            Event::RelayKeySaved { set, ok } => {
                if !ok {
                    self.toast(
                        ToastKind::Error,
                        "The access key could not be saved in the system's credential store.",
                    );
                    return;
                }
                self.settings.relay_key_set = set;
                self.mark_settings_dirty();
                if set {
                    self.form_relay_key.clear();
                }
                // Reconnect every account with the new key.
                self.send_relay();
            }
            Event::Plugin {
                id,
                generation,
                event,
            } => self.on_plugin_event(id, generation, event),
            Event::PluginSecretSaved {
                plugin,
                instance,
                option,
                set,
                ok,
            } => self.on_plugin_secret_saved(&plugin, &instance, option, set, ok),
        }
    }

    fn on_auth(&mut self, status: AuthStatus) {
        log::info!("account: {}", status.log_label());
        let became_signed_in = matches!(status, AuthStatus::SignedIn { .. })
            && !matches!(self.auth, AuthStatus::SignedIn { .. });
        let signed_in_account = match &status {
            AuthStatus::SignedIn { account, .. } if became_signed_in => Some(account.clone()),
            _ => None,
        };
        match &status {
            AuthStatus::SignedIn { account, .. } => {
                // The same account signed in twice (through "Add account"):
                // keep the slot that had it and show that one.
                if let Some(existing) =
                    self.accounts.iter().enumerate().position(|(index, a)| {
                        index != self.active && a.id.as_deref() == Some(account)
                    })
                {
                    self.retire.push(self.active);
                    self.adding_from = None;
                    self.toast(ToastKind::Info, "That account is already signed in.");
                    self.actions
                        .push(Action::SwitchAccount(if existing > self.active {
                            existing - 1
                        } else {
                            existing
                        }));
                    return;
                }
                self.id = Some(account.clone());
                self.reauth = None;
                self.remember_browser_profile();
                if !self.hidden {
                    self.adding_from = None;
                }
            }
            AuthStatus::SignedOut => {
                self.clear_data();
                // The account has no presence now; plugins hear it.
                self.sync_plugin_presence();
            }
            AuthStatus::Failed(reason) => {
                // Signing in again did not finish: back to the cached
                // workspace, with the reason.
                if let Some(AuthStatus::Expired { username, .. }) = self.reauth.take() {
                    self.auth = AuthStatus::Expired {
                        username,
                        reason: reason.clone(),
                    };
                    return;
                }
            }
            AuthStatus::Expired { username, reason } => {
                // Keep the name the account had when Graph does not repeat it.
                if username.is_empty()
                    && let AuthStatus::SignedIn {
                        username: known, ..
                    } = &self.auth
                {
                    let known = known.clone();
                    let reason = reason.clone();
                    self.auth = AuthStatus::Expired {
                        username: known,
                        reason,
                    };
                    return;
                }
            }
            _ => {}
        }
        self.auth = status;
        if !self.hidden {
            self.continue_to_sign_in();
        }
        if became_signed_in {
            if let Some(account) = &signed_in_account {
                self.on_signed_in_setup(account);
            }
            self.remember_accounts_for_slot();
            for resource in [
                Resource::Profile,
                Resource::Chats,
                Resource::Teams,
                Resource::Calendar,
                Resource::MyPresence,
            ] {
                self.fetch(resource, Trigger::Start);
            }
            if let Some(open) = &self.open {
                self.fetch(Resource::Messages(open.clone()), Trigger::Start);
            }
        }
    }

    /// `remember_accounts` from inside event handling, where the active
    /// index may point at a hidden account.
    fn remember_accounts_for_slot(&mut self) {
        let ids: Vec<String> = self.accounts.iter().filter_map(|a| a.id.clone()).collect();
        if self.settings.accounts != ids {
            self.settings.accounts = ids;
            self.mark_settings_dirty();
        }
        if !self.hidden && self.settings.account != self.id {
            self.settings.account = self.id.clone();
            self.mark_settings_dirty();
        }
        self.sync_plugin_accounts();
    }

    fn on_chats(&mut self, mut chats: Vec<Chat>, cached: bool) {
        // A chat being left stays out of the list until Graph answers.
        if !cached {
            // Once Graph stops listing a left chat, forget it.
            let listed: HashSet<&str> = chats.iter().map(|c| c.id.as_str()).collect();
            self.left.retain(|id| listed.contains(id.as_str()));
        }
        chats.retain(|c| !self.leaving.contains_key(&c.id) && !self.left.contains(&c.id));
        chats.sort_by_key(Chat::list_order);
        let me = self.me().to_owned();
        if !cached && !self.chats.is_empty() {
            let mut notices = Vec::new();
            for chat in &chats {
                let Some(preview) = &chat.preview else {
                    continue;
                };
                let previous = self.chat(&chat.id).and_then(|c| c.preview.as_ref());
                let new = previous
                    .is_none_or(|p| p.message_id != preview.message_id && preview.at > p.at);
                let looking = !self.hidden
                    && self.window_focused
                    && self.open == Some(ConversationId::Chat(chat.id.clone()));
                if new
                    && !looking
                    && preview.author_id.as_deref() != Some(me.as_str())
                    && !preview.deleted
                {
                    notices.push((
                        chat.title(&me),
                        preview.author.clone(),
                        preview.text.clone(),
                    ));
                }
            }
            for (title, author, text) in notices {
                self.notify_message(&title, author.as_deref(), &text);
            }
        }
        // Read state Chaat set locally survives a cached list.
        for chat in &mut chats {
            if let Some(known) = self.chats.iter().find(|c| c.id == chat.id)
                && known.last_read > chat.last_read
            {
                chat.last_read = known.last_read;
            }
        }
        self.chats = chats;
    }

    fn notify_message(&self, title: &str, author: Option<&str>, text: &str) {
        if !self.settings.notifications || self.demo {
            return;
        }
        let body = match author {
            Some(author) if !title.contains(author) => format!("{author}: {text}"),
            _ => text.to_owned(),
        };
        // With several accounts, say which one the message is for.
        let title = if self.accounts.len() > 1 {
            format!("{title} · {}", self.username())
        } else {
            title.to_owned()
        };
        crate::notify::show(&title, &body);
    }

    fn on_messages(
        &mut self,
        conversation: ConversationId,
        messages: Vec<Message>,
        merge: Merge,
        more: Option<bool>,
        cached: bool,
    ) {
        if !self.recent_conversations.contains(&conversation) {
            self.recent_conversations.push_front(conversation.clone());
        }
        let me = self.me().to_owned();
        let open = !self.hidden && self.open.as_ref() == Some(&conversation);
        let focused = self.window_focused;
        let entry = self.conversations.entry(conversation.clone()).or_default();
        // The disk-only reopen can finish after Graph. Do not replace a
        // newer network answer with a snapshot read before that answer.
        if cached && merge == Merge::Latest && entry.fresh {
            return;
        }
        let newest_before = entry
            .messages
            .iter()
            .filter(|m| m.delivery == Delivery::Sent)
            .map(|m| m.created)
            .max();
        let incoming_new: Vec<(String, String)> = messages
            .iter()
            .filter(|m| !m.system && !m.deleted && !m.is_from(&me))
            .filter(|m| newest_before.is_some_and(|newest| m.created > newest))
            .map(|m| (m.author.clone(), m.body.summary(140)))
            .collect();
        if merge == Merge::Older && !messages.is_empty() {
            entry.restore_history_anchor = true;
        }
        entry.merge(messages, merge);
        if let Some(more) = more {
            // A cached page says history may exist. It must not undo an
            // end-of-history answer Graph already gave in this process.
            if !cached || !entry.fresh {
                entry.more = more;
            }
        }
        if !cached {
            entry.fresh = true;
        }
        if !cached && !(open && focused) {
            let title = self.title_of(&conversation);
            for (author, text) in incoming_new.iter().take(3) {
                self.notify_message(&title, Some(author), text);
            }
        }
        // A message that arrives while Chaat is behind another window stays
        // unread, so Teams keeps it unread on other devices too. Coming back
        // to the window marks it read.
        if open
            && focused
            && !cached
            && let ConversationId::Chat(chat) = &conversation
        {
            self.mark_read(&chat.clone());
        }
        // Keep the chat list's preview in step with what the chat shows.
        // People added or removed don't count, as in Teams: they would move
        // the chat up the list.
        if let ConversationId::Chat(chat_id) = &conversation
            && let Some(last) = self
                .conversations
                .get(&conversation)
                .and_then(|c| c.messages.iter().rev().find(|m| m.membership.is_none()))
                .cloned()
            && let Some(chat) = self.chats.iter_mut().find(|c| &c.id == chat_id)
            && chat
                .preview
                .as_ref()
                .is_none_or(|p| p.at < last.created || p.membership.is_some())
            && last.delivery == Delivery::Sent
        {
            chat.preview = Some(crate::model::Preview {
                message_id: last.id.clone(),
                author_id: last.author_id.clone(),
                author: Some(last.author.clone()),
                text: preview_text(&last),
                at: last.created,
                deleted: last.deleted,
                system: last.system,
                membership: last.membership.clone(),
            });
            self.chats.sort_by_key(Chat::list_order);
        }
        self.trim_conversations();
    }

    fn on_sent(&mut self, conversation: ConversationId, local_id: String, message: Message) {
        self.pending_sends.remove(&local_id);
        let entry = self.conversations.entry(conversation.clone()).or_default();
        if let Some(reply_to) = &message.reply_to
            && conversation.is_channel()
        {
            if let Some(root) = entry.messages.iter_mut().find(|m| &m.id == reply_to) {
                root.replies.retain(|r| r.id != local_id);
                entry.heights.remove(reply_to);
                entry.layout.invalidate();
                root.replies.push(message);
            }
            return;
        }
        entry.messages.retain(|m| m.id != local_id);
        entry.merge(vec![message], Merge::Changes);
        self.fetch(Resource::Messages(conversation), Trigger::AfterAction);
    }

    fn mark_read(&mut self, chat: &str) {
        if self.hidden {
            return;
        }
        let me = self.me().to_owned();
        let demo = self.demo;
        let Some(entry) = self.chats.iter_mut().find(|c| c.id == chat) else {
            return;
        };
        if !entry.is_unread(&me) {
            return;
        }
        entry.last_read = Some(crate::time::now());
        if !demo {
            self.backend.send(Command::MarkRead(chat.to_owned()));
        }
    }

    // -- Actions -------------------------------------------------------------------

    pub fn apply_actions(&mut self, ctx: &egui::Context) {
        let mut rounds = 0;
        while !self.actions.is_empty() && rounds < 8 {
            rounds += 1;
            for action in std::mem::take(&mut self.actions) {
                self.apply(action, ctx);
            }
        }
    }

    pub fn apply(&mut self, action: Action, ctx: &egui::Context) {
        self.apply_one(action, ctx);
        self.plugins_view_follows_settings();
    }

    fn apply_one(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::CheckUpdate => self.check_update(self.settings.automatic_updates),
            Action::SetAutomaticUpdates(enabled) => {
                self.settings.automatic_updates = enabled;
                self.mark_settings_dirty();
            }
            Action::DownloadUpdate => {
                if let crate::updates::Status::Available {
                    release,
                    reason: None,
                } = &self.update
                {
                    self.service.send(Command::DownloadUpdate(release.clone()));
                    self.update = crate::updates::Status::Downloading {
                        received: 0,
                        total: 0,
                    };
                }
            }
            Action::InstallUpdate => {
                let unsent = self.accounts.iter().any(|account| {
                    !account.pending_sends.is_empty()
                        || account.drafts.values().any(|draft| {
                            !draft.text.is_empty()
                                || !draft.files.is_empty()
                                || !draft.images.is_empty()
                                || draft.pending_pastes > 0
                        })
                });
                if unsent {
                    self.toast(ToastKind::Info, "Send or clear your drafts and finish pending sends before restarting to update.");
                } else if matches!(self.update, crate::updates::Status::Ready(_)) {
                    let crate::updates::Status::Ready(prepared) =
                        std::mem::replace(&mut self.update, crate::updates::Status::Installing)
                    else {
                        unreachable!()
                    };
                    self.service.send(Command::InstallUpdate(prepared));
                }
            }
            Action::Navigate(section) => {
                self.settings_page = None;
                if self.section != section {
                    self.section = section;
                    self.open = match (section, &self.open) {
                        (Section::Chats, Some(open @ ConversationId::Chat(_))) => {
                            Some(open.clone())
                        }
                        (Section::Teams, Some(open @ ConversationId::Channel { .. })) => {
                            Some(open.clone())
                        }
                        _ => None,
                    };
                }
                self.fetch_view(Trigger::Opened);
            }
            Action::OpenConversation(conversation) => self.open_conversation(conversation),
            Action::OpenMessage {
                conversation,
                message,
            } => {
                self.open_conversation(conversation);
                self.jump_to = Some(message);
                self.overlay = None;
            }
            Action::ToggleTeam(team) => {
                if !self.expanded_teams.remove(&team) {
                    self.expanded_teams.insert(team.clone());
                    if self.channels.get(&team).is_none_or(Vec::is_empty) {
                        self.fetch(Resource::Channels(team), Trigger::Opened);
                    }
                }
            }
            Action::CloseConversation => self.open = None,
            Action::Step(delta) => self.step(delta),
            Action::Send {
                conversation,
                text,
                reply_to,
                mentions,
                files,
                images,
            } => self.send(conversation, text, reply_to, mentions, files, images),
            Action::Edit {
                conversation,
                message,
                text,
            } => {
                let html = html::compose(&text);
                if let Some(entry) = self.conversations.get_mut(&conversation)
                    && let Some(found) = entry.find_mut(&message)
                {
                    found.body = html::parse(&html);
                    found.edited = true;
                }
                if !self.demo {
                    self.backend.send(Command::Edit {
                        conversation: conversation.clone(),
                        message,
                        html,
                    });
                }
                self.drafts.remove(&conversation);
            }
            Action::Delete {
                conversation,
                message,
            } => {
                if let Some(entry) = self.conversations.get_mut(&conversation)
                    && let Some(found) = entry.find_mut(&message)
                {
                    found.deleted = true;
                    found.body = html::RichText::default();
                }
                if !self.demo {
                    self.backend.send(Command::Delete {
                        conversation,
                        message,
                    });
                }
            }
            Action::React {
                conversation,
                message,
                emoji,
                on,
            } => {
                let me = self.me().to_owned();
                if let Some(entry) = self.conversations.get_mut(&conversation)
                    && let Some(found) = entry.find_mut(&message)
                {
                    toggle_reaction(&mut found.reactions, &emoji, &me, on);
                }
                if !self.demo {
                    self.backend.send(Command::React {
                        conversation,
                        message,
                        emoji,
                        on,
                    });
                }
            }
            Action::StartReply(message) => {
                if let Some(open) = self.open.clone() {
                    let draft = self.drafts.entry(open).or_default();
                    draft.reply_to = Some(message);
                    draft.editing = None;
                    self.focus_composer = true;
                }
            }
            Action::StartEdit(message) => {
                if let Some(open) = self.open.clone() {
                    let text = self
                        .conversations
                        .get(&open)
                        .and_then(|c| c.find(&message))
                        .map(|m| m.body.to_plain())
                        .unwrap_or_default();
                    self.drafts.insert(
                        open,
                        Draft {
                            text,
                            editing: Some(message),
                            ..Default::default()
                        },
                    );
                    self.focus_composer = true;
                }
            }
            Action::CancelCompose => {
                if let Some(open) = self.open.clone()
                    && let Some(draft) = self.drafts.get_mut(&open)
                {
                    if draft.editing.take().is_some() {
                        draft.text.clear();
                    }
                    draft.reply_to = None;
                }
            }
            Action::LoadOlder(conversation) => {
                if !self.demo {
                    let resource = Resource::Messages(conversation.clone());
                    if self.loading.contains(&resource) {
                        return;
                    }
                    let before = self.conversations.get(&conversation).and_then(|c| {
                        c.messages
                            .iter()
                            .filter(|m| m.delivery == Delivery::Sent)
                            .map(|m| m.created)
                            .min()
                    });
                    self.loading.insert(resource);
                    self.backend.send(Command::LoadOlder {
                        conversation,
                        before,
                    });
                } else if let Some(entry) = self.conversations.get_mut(&conversation) {
                    entry.more = false;
                }
            }
            Action::LoadReplies {
                conversation,
                message,
            } => {
                if !self.demo {
                    self.backend.send(Command::LoadReplies {
                        conversation,
                        root: message,
                    });
                }
            }
            Action::ToggleThread(root) => {
                if let Some(open) = self.open.clone() {
                    let entry = self.conversations.entry(open).or_default();
                    entry.heights.remove(&root);
                    if !entry.expanded.remove(&root) {
                        entry.expanded.insert(root);
                    }
                    entry.layout.invalidate();
                }
            }
            Action::RetrySend {
                conversation,
                local_id,
            } => {
                if let Some(pending) = self.pending_sends.get(&local_id).cloned() {
                    if let Some(entry) = self.conversations.get_mut(&conversation)
                        && let Some(found) = entry.find_mut(&local_id)
                    {
                        found.delivery = Delivery::Sending;
                    }
                    self.backend.send(Command::Send {
                        conversation,
                        local_id,
                        html: pending.html,
                        reply_to: pending.reply_to,
                        mentions: pending.mentions,
                        files: pending.files,
                        images: pending.images,
                        recipients: pending.recipients,
                    });
                }
            }
            Action::AttachFiles => {
                if self.open.is_none() || !self.ensure_scope(crate::auth::FILES_SCOPE) {
                    return;
                }
                // The platform's own picker; it is modal, like a dialog.
                if let Some(paths) = rfd::FileDialog::new()
                    .set_title("Attach files")
                    .pick_files()
                {
                    self.actions.push(Action::AddFiles(paths));
                }
            }
            Action::Paste {
                conversation,
                range,
                text,
            } => {
                self.drafts
                    .entry(conversation.clone())
                    .or_default()
                    .pending_pastes += 1;
                self.backend.send(Command::Paste {
                    conversation,
                    range,
                    text,
                });
            }
            Action::AddFiles(paths) => self.add_files(paths),
            Action::RemoveFile(index) => {
                if let Some(open) = self.open.clone()
                    && let Some(draft) = self.drafts.get_mut(&open)
                    && index < draft.files.len()
                {
                    draft.files.remove(index);
                }
            }
            Action::LoadMedia(url) => {
                let missing = match self.media.get(&url) {
                    Some(Image::Ready(uri)) => !crate::images::contains(ctx, uri),
                    None => true,
                    _ => false,
                };
                if missing {
                    let demo = self.demo;
                    self.media.insert(url.clone(), Image::Loading);
                    if demo {
                        self.media.insert(url, Image::Missing);
                    } else {
                        self.backend.send(Command::Media(url));
                    }
                }
            }
            Action::SignIn => {
                if matches!(self.auth, AuthStatus::Expired { .. }) {
                    self.reauth = Some(self.auth.clone());
                }
                self.backend.send(Command::SignIn {
                    extra_scopes: Vec::new(),
                    device_code: false,
                });
            }
            Action::SignInWithCode => self.backend.send(Command::SignIn {
                extra_scopes: Vec::new(),
                device_code: true,
            }),
            Action::CancelSignIn => {
                self.backend.send(Command::CancelSignIn);
                self.auth = self.reauth.take().unwrap_or(AuthStatus::SignedOut);
            }
            Action::SignOut => self.sign_out(),
            Action::RequestScope(scope) => {
                self.overlay = None;
                if !self.demo {
                    self.backend.send(Command::SignIn {
                        extra_scopes: vec![scope.to_owned()],
                        device_code: false,
                    });
                    self.toast(
                        ToastKind::Info,
                        "Approve the permission in your browser, then try again.",
                    );
                }
            }
            Action::SaveAppRegistration { client_id, tenant } => {
                let client_id = client_id.trim().to_owned();
                if !crate::settings::is_guid(&client_id) {
                    self.toast(
                        ToastKind::Error,
                        "The application (client) ID is a GUID like 00000000-0000-0000-0000-000000000000.",
                    );
                    return;
                }
                self.onboarding_forced = false;
                self.settings.client_id = client_id;
                self.settings.tenant = match tenant.trim() {
                    "" => "organizations".into(),
                    other => other.to_owned(),
                };
                self.mark_settings_dirty();
                self.save_settings();
                if !self.demo {
                    for index in 0..self.accounts.len() {
                        self.start_account(index);
                    }
                }
            }
            Action::SetAvailability(availability) => {
                if !self.ensure_scope(crate::auth::PRESENCE_WRITE_SCOPE) {
                    return;
                }
                // Shown at once; Graph's answer replaces it.
                let shown = Presence {
                    availability: availability.unwrap_or(Availability::Available),
                    activity: None,
                    status_message: self
                        .my_presence
                        .as_ref()
                        .and_then(|p| p.status_message.clone()),
                };
                let me = self.me().to_owned();
                self.presence.insert(me, shown.clone());
                self.my_presence = Some(shown);
                self.sync_plugin_presence();
                if !self.demo {
                    self.backend.send(Command::SetAvailability(availability));
                }
            }
            Action::OpenStatusMessage => {
                if !self.ensure_scope(crate::auth::PRESENCE_WRITE_SCOPE) {
                    return;
                }
                self.form_status = self
                    .my_presence
                    .as_ref()
                    .and_then(|p| p.status_message.clone())
                    .unwrap_or_default();
                self.overlay = Some(Overlay::StatusMessage);
            }
            Action::SetStatusMessage(text) => {
                self.overlay = None;
                let text = text.trim().to_owned();
                let mut presence = self.my_presence.clone().unwrap_or_default();
                presence.status_message = (!text.is_empty()).then(|| text.clone());
                let me = self.me().to_owned();
                self.presence.insert(me, presence.clone());
                self.my_presence = Some(presence);
                self.sync_plugin_presence();
                if !self.demo {
                    self.backend.send(Command::SetStatusMessage(text));
                }
            }
            Action::OpenOutOfOffice => {
                self.apply(Action::ShowSettingsPage(SettingsPage::OutOfOffice), ctx);
                self.ensure_scope(crate::auth::MAILBOX_SETTINGS_SCOPE);
            }
            Action::SetOutOfOffice(value) => {
                if self.demo {
                    self.form_out_of_office =
                        crate::model::OutOfOfficeForm::new(&value, crate::time::now());
                    self.out_of_office = Some(value);
                } else if !self.out_of_office_saving {
                    self.out_of_office_saving = true;
                    self.backend.send(Command::SetOutOfOffice(value));
                }
            }
            Action::SetPresenceSession(on) => {
                self.settings.presence_session = on;
                self.mark_settings_dirty();
                for account in &self.accounts {
                    account.backend.send(Command::PresenceSession(on));
                }
            }
            Action::SwitchAccount(index) => {
                if index < self.accounts.len() && index != self.active {
                    self.active = index;
                    self.overlay = None;
                    self.switcher = Switcher::default();
                    self.scroll_to_end = 4;
                    self.remember_accounts();
                    self.fetch_view(Trigger::Focused);
                    self.fetch(Resource::MyPresence, Trigger::Focused);
                    if self.settings_page == Some(SettingsPage::OutOfOffice) {
                        self.show_out_of_office();
                    }
                }
            }
            Action::AddAccount => {
                if self.demo {
                    self.toast(
                        ToastKind::Info,
                        "The demo has its accounts already: switch between them in this menu.",
                    );
                    return;
                }
                self.overlay = None;
                let from = self.active;
                let index = self.add_slot(None);
                self.adding_from = Some(from);
                self.active = index;
                if self.settings.has_app_registration() {
                    // Straight to Microsoft's account picker.
                    self.backend.send(Command::SignIn {
                        extra_scopes: Vec::new(),
                        device_code: false,
                    });
                }
            }
            Action::CancelAddAccount => {
                if let Some(from) = self.adding_from.take() {
                    self.backend.send(Command::CancelSignIn);
                    let adding = self.active;
                    self.retire_slot(adding);
                    self.active = if from > adding { from - 1 } else { from }
                        .min(self.accounts.len().saturating_sub(1));
                }
            }
            Action::OpenNewChat => {
                if !self.ensure_scope(crate::auth::CHAT_CREATE_SCOPE) {
                    return;
                }
                self.new_chat = NewChat::default();
                self.people = People::default();
                self.overlay = Some(Overlay::NewChat);
            }
            Action::OpenAddPeople(chat) => {
                self.add_people = AddPeople::default();
                self.people = People::default();
                self.overlay = Some(Overlay::AddPeople(chat));
            }
            Action::AddMembers {
                chat,
                members,
                history,
            } => {
                let existing: Vec<String> = self
                    .chats
                    .iter()
                    .find(|c| c.id == chat)
                    .map(|c| c.members.iter().map(|m| m.user_id.clone()).collect())
                    .unwrap_or_default();
                let mut fresh: Vec<Member> = Vec::new();
                for member in members {
                    if !existing.contains(&member.user_id)
                        && !fresh.iter().any(|m| m.user_id == member.user_id)
                    {
                        fresh.push(member);
                    }
                }
                self.overlay = None;
                if fresh.is_empty() {
                    return;
                }
                let count = fresh.len();
                let users: Vec<String> = fresh.iter().map(|m| m.user_id.clone()).collect();
                if let Some(found) = self.chats.iter_mut().find(|c| c.id == chat) {
                    found.members.extend(fresh);
                }
                if self.demo {
                    let noun = if count == 1 { "person" } else { "people" };
                    self.toast(ToastKind::Info, format!("Added {count} {noun}"));
                } else {
                    for user in &users {
                        self.adding.insert((chat.clone(), user.clone()));
                    }
                    self.backend.send(Command::AddMembers {
                        chat,
                        users,
                        history,
                    });
                }
            }
            Action::OpenLeaveChat(chat) => self.overlay = Some(Overlay::LeaveChat(chat)),
            Action::LeaveChat(chat) => {
                if !self.demo && !self.has_scope(crate::auth::CHAT_MEMBER_SCOPE) {
                    // Never delete without the permission: explain it instead.
                    self.overlay = Some(Overlay::LeaveChat(chat));
                    return;
                }
                self.overlay = None;
                if self.open == Some(ConversationId::Chat(chat.clone())) {
                    self.open = None;
                }
                if self.demo {
                    self.chats.retain(|c| c.id != chat);
                } else if let Some(found) = self.chats.iter().position(|c| c.id == chat) {
                    let found = self.chats.remove(found);
                    self.leaving.insert(chat.clone(), found);
                    self.backend.send(Command::LeaveChat(chat));
                }
            }
            Action::PeopleOpened(chat) => {
                self.fetch(Resource::Members(chat), Trigger::Opened);
            }
            Action::SearchPeople(query) => {
                self.people.query = query.clone();
                self.people.results.clear();
                if query.trim().chars().count() < 2 {
                    return;
                }
                if self.demo {
                    self.people.results = crate::demo::search_people(self, &query);
                } else {
                    self.backend.send(Command::SearchPeople(query));
                }
            }
            Action::CreateChat { members, topic } => self.create_chat(members, topic),
            Action::JoinMeeting(url) => self.join_meeting(url, ctx),
            Action::StartCall { users, video } => self.start_call(users, video, ctx),
            Action::CallDone => self.end_call(ctx),
            Action::ShowCall => self.show_call(),
            Action::DockWindow(true) => {
                self.dock(ctx);
            }
            Action::DockWindow(false) => self.undock(ctx),
            Action::FindMeetingChat => self.find_meeting_chat(),
            action @ (Action::ChooseSetupFile
            | Action::DroppedSetupFile(_)
            | Action::OpenSetupFile
            | Action::ApplySetup
            | Action::CancelSetup
            | Action::ShowSetupManual(_)) => self.setup_action(action),
            action @ (Action::OpenExportForm(_)
            | Action::ExportSetup
            | Action::DismissSetupPassphrase) => self.export_action(action),
            Action::JoinOnPhone => self.join_on_phone(),
            Action::PrepareTeamsWeb => self.prepare_teams_web(),
            Action::SetCallSettings(calls) => self.set_call_settings(calls),
            Action::OpenUrl(url) => {
                if url.starts_with("https://") || url.starts_with("http://") {
                    self.open_link(&url);
                } else if url.starts_with("mailto:") {
                    open_external(&url);
                }
            }
            Action::CopyText(text) => {
                ctx.copy_text(text);
                self.toast(ToastKind::Info, "Copied");
            }
            Action::OpenSwitcher => {
                self.switcher = Switcher::default();
                self.overlay = Some(Overlay::Switcher);
            }
            Action::CloseOverlay => {
                self.overlay = None;
                // Give loaders a pass without the original so they can release it.
                ctx.request_repaint();
            }
            Action::PreviewImage(uri) => {
                if !crate::images::contains(ctx, &uri) {
                    let local = self
                        .accounts
                        .iter()
                        .flat_map(|a| {
                            a.drafts
                                .values()
                                .flat_map(|d| &d.images)
                                .chain(a.pending_sends.values().flat_map(|s| &s.images))
                        })
                        .find(|image| image.uri() == uri)
                        .cloned();
                    if let Some(image) = local {
                        crate::images::include(ctx, uri.clone(), image.png);
                    } else if let Some(url) = self.media.iter().find_map(|(url, state)| {
                        matches!(state, Image::Ready(stored) if *stored == uri).then(|| url.clone())
                    }) {
                        self.apply(Action::LoadMedia(url), ctx);
                    }
                }
                self.overlay = Some(Overlay::ImagePreview(uri));
            }
            Action::SearchMessages(query) => {
                self.switcher.searched = query.clone();
                self.switcher.hits.clear();
                self.switcher.remote_hits.clear();
                if query.trim().is_empty() {
                    return;
                }
                if self.demo {
                    self.switcher.hits = self.local_search(&query);
                } else {
                    self.backend.send(Command::Search(query));
                }
            }
            Action::OpenSettings => {
                let page = self.last_settings_page;
                self.apply(Action::ShowSettingsPage(page), ctx);
            }
            Action::OpenShortcuts => {
                self.apply(Action::ShowSettingsPage(SettingsPage::Shortcuts), ctx)
            }
            Action::ShowSettingsPage(page) => {
                self.settings_page = Some(page);
                self.last_settings_page = page;
                self.overlay = None;
                if page == SettingsPage::OutOfOffice {
                    self.show_out_of_office();
                }
            }
            Action::CloseSettings => self.settings_page = None,
            Action::SetTheme(choice) => {
                self.settings.theme = choice;
                self.mark_settings_dirty();
            }
            Action::SetNotifications(on) => {
                self.settings.notifications = on;
                self.mark_settings_dirty();
            }
            Action::SetKeepRunning(on) => {
                self.settings.keep_running = on;
                self.mark_settings_dirty();
                // The tray is the way back to a closed window: make it now.
                if on && self.tray.is_none() && !self.demo {
                    self.tray = crate::platform::tray(&self.waker);
                    if let Some(tray) = &mut self.tray {
                        tray.attach();
                    }
                }
            }
            Action::SetCustomTitlebar(on) => {
                self.settings.custom_titlebar = on;
                self.mark_settings_dirty();
                fastframe_titlebar::set_enabled(on);
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(
                    !fastframe_titlebar::enabled(),
                ));
            }
            Action::SetRelay(url) => {
                let url = url.trim().to_owned();
                if !url.is_empty() && crate::realtime::normalize_relay(&url).is_none() {
                    self.toast(ToastKind::Error, "The relay must be an https:// address.");
                    return;
                }
                self.settings.relay_url = (!url.is_empty()).then_some(url);
                self.mark_settings_dirty();
                self.send_relay();
                // A manual relay wins; clearing it brings the plugin back.
                self.sync_push_route(false);
            }
            Action::SetRelayKey(key) => {
                let key = key.map(|key| key.trim().to_owned());
                if key
                    .as_deref()
                    .is_some_and(|key| !crate::realtime::valid_relay_key(key))
                {
                    self.toast(
                        ToastKind::Error,
                        "The access key must be at least 16 characters, without spaces.",
                    );
                    return;
                }
                // The service backend owns the credential store write, so
                // accounts never write the shared entry at the same time.
                if !self.demo {
                    self.service
                        .send(Command::SaveRelayKey(key.map(crate::realtime::RelayKey)));
                }
            }
            Action::LookUpNames(users) => {
                // Asked once each; the demo has no directory to ask.
                let demo = self.demo;
                for user in users {
                    if self.person_name(&user).is_none()
                        && self.names_asked.insert(user.clone())
                        && !demo
                    {
                        self.backend.send(Command::UserName(user));
                    }
                }
            }
            Action::RetryRelay => {
                // Accounts already connected keep their connection.
                if !self.demo {
                    for account in &self.accounts {
                        if matches!(account.push, crate::realtime::PushStatus::Failed(_)) {
                            account
                                .backend
                                .send(Command::Relay(self.settings.relay_url.clone()));
                        }
                    }
                }
            }
            Action::TogglePerfOverlay => {
                self.settings.perf_overlay = !self.settings.perf_overlay;
                self.mark_settings_dirty();
            }
            Action::Refresh => {
                self.fetch_view(Trigger::Refresh);
                if self.demo {
                    self.toast(ToastKind::Info, "The demo has no account to refresh from.");
                }
            }
            Action::DismissToast(id) => self.toasts.retain(|t| t.id != id),
            Action::DismissRing(id) => self.rings.retain(|r| r.id != id),
            action @ (Action::AddPlugin
            | Action::RemovePlugin(_)
            | Action::SetPluginEnabled(..)
            | Action::ApprovePluginNeeds(_)
            | Action::DenyPluginNeeds(_)
            | Action::TrustPluginFile(_)
            | Action::KeepPluginOff(_)
            | Action::SetPluginOption { .. }
            | Action::RunPluginAction { .. }
            | Action::ResetPluginSection { .. }
            | Action::OpenPlugin(_)
            | Action::BackToPlugins
            | Action::StartReorder
            | Action::EndReorder
            | Action::MovePlugin { .. }
            | Action::FinishPluginSetup { .. }
            | Action::CancelPluginSetup(_)
            | Action::RetryPluginStart(_)
            | Action::SetPluginSecret { .. }
            | Action::PluginSignIn { .. }
            | Action::HidePluginSignIn { .. }) => self.apply_plugin_action(action),
            Action::RingPlaceCall(id) => self.ring_place_call(&id, ctx),
            Action::RingOpenChat(id) => self.ring_open_chat(&id, ctx),
            Action::Quit => {
                self.quit_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn open_conversation(&mut self, conversation: ConversationId) {
        self.settings_page = None;
        let me = self.me().to_owned();
        self.unread_kept = match &conversation {
            ConversationId::Chat(id) if self.chat(id).is_some_and(|c| c.is_unread(&me)) => {
                Some(id.clone())
            }
            _ => None,
        };
        let needs_cache = !self.conversations.contains_key(&conversation);
        // Before marking it read: where the "Last read" divider goes.
        let unread_after = match &conversation {
            ConversationId::Chat(id) => self
                .chat(id)
                .filter(|c| c.is_unread(&me))
                .and_then(|c| c.last_read),
            ConversationId::Channel { .. } => None,
        };
        if let Some(entry) = self.conversations.get_mut(&conversation) {
            entry.set_unread_after(unread_after);
        } else if unread_after.is_some() {
            self.conversations
                .entry(conversation.clone())
                .or_default()
                .set_unread_after(unread_after);
        }
        self.recent_conversations.retain(|id| id != &conversation);
        self.recent_conversations.push_back(conversation.clone());
        self.section = match conversation {
            ConversationId::Chat(_) => Section::Chats,
            ConversationId::Channel { .. } => Section::Teams,
        };
        if let ConversationId::Channel { team, .. } = &conversation {
            self.expanded_teams.insert(team.clone());
        }
        self.open = Some(conversation.clone());
        self.trim_conversations();
        if needs_cache && !self.demo {
            self.backend
                .send(Command::CachedMessages(conversation.clone()));
        }
        self.focus_composer = true;
        self.scroll_to_end = 4;
        self.fetch(Resource::Messages(conversation.clone()), Trigger::Opened);
        if let ConversationId::Chat(chat) = &conversation {
            self.mark_read(chat);
            let me = self.me().to_owned();
            let members: Vec<String> = self
                .chat(chat)
                .map(|c| {
                    c.members
                        .iter()
                        .filter(|m| m.user_id != me)
                        .map(|m| m.user_id.clone())
                        .take(20)
                        .collect()
                })
                .unwrap_or_default();
            if !members.is_empty() && !self.demo {
                self.backend.send(Command::Presence(members));
            }
        }
    }

    /// SQLite keeps the full history. RAM keeps the current conversation,
    /// pending/failed sends, and a small set of recently opened conversations.
    fn trim_conversations(&mut self) {
        if self.demo {
            return;
        }
        loop {
            let inactive_messages: usize = self
                .conversations
                .iter()
                .filter(|(id, _)| self.open.as_ref() != Some(*id))
                .map(|(_, view)| {
                    view.messages
                        .iter()
                        .map(|message| 1 + message.replies.len())
                        .sum::<usize>()
                })
                .sum();
            if self.conversations.len() <= 12 && inactive_messages <= 2_000 {
                break;
            }
            let oldest = self
                .recent_conversations
                .iter()
                .find(|id| {
                    self.open.as_ref() != Some(*id)
                        && self.conversations.get(*id).is_some_and(|view| {
                            view.messages.iter().all(|m| {
                                m.delivery == Delivery::Sent
                                    && m.replies.iter().all(|r| r.delivery == Delivery::Sent)
                            })
                        })
                })
                .cloned();
            let Some(oldest) = oldest else { break };
            self.conversations.remove(&oldest);
            self.recent_conversations.retain(|id| id != &oldest);
        }
    }

    fn step(&mut self, delta: i32) {
        let me = self.me().to_owned();
        let list: Vec<ConversationId> = match self.section {
            Section::Chats => self
                .chats
                .iter()
                .filter(|c| crate::ui::chat_list::listed(self, c, &me))
                .map(|c| ConversationId::Chat(c.id.clone()))
                .collect(),
            Section::Teams => self
                .teams
                .iter()
                .flat_map(|t| self.channels.get(&t.id).into_iter().flatten())
                .map(|c| ConversationId::Channel {
                    team: c.team_id.clone(),
                    channel: c.id.clone(),
                })
                .collect(),
            _ => return,
        };
        if list.is_empty() {
            return;
        }
        let current = self
            .open
            .as_ref()
            .and_then(|open| list.iter().position(|c| c == open));
        let next = match current {
            Some(index) => (index as i32 + delta).clamp(0, list.len() as i32 - 1) as usize,
            None => 0,
        };
        self.open_conversation(list[next].clone());
    }

    fn check_update(&mut self, download: bool) {
        if self.update.busy() || matches!(self.update, crate::updates::Status::Ready(_)) {
            return;
        }
        self.update = crate::updates::Status::Checking;
        self.settings.last_update_check = crate::time::now();
        self.mark_settings_dirty();
        self.service.send(Command::CheckUpdate { download });
    }

    fn add_files(&mut self, paths: Vec<PathBuf>) {
        let Some(open) = self.open.clone() else {
            self.toast(ToastKind::Info, "Open a chat or channel to send files to.");
            return;
        };
        if !self.ensure_scope(crate::auth::FILES_SCOPE) {
            return;
        }
        let mut refused = Vec::new();
        let draft = self.drafts.entry(open).or_default();
        for path in paths {
            let size = std::fs::metadata(&path).map(|m| (m.is_file(), m.len()));
            match size {
                Ok((true, size)) if size <= crate::graph::files::MAX_FILE => {
                    if !draft.files.contains(&path) {
                        draft.files.push(path);
                    }
                }
                _ => refused.push(
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            }
        }
        self.focus_composer = true;
        if !refused.is_empty() {
            self.toast(
                ToastKind::Error,
                format!(
                    "Not attached (folders and files over 1 GB are not sent): {}",
                    refused.join(", ")
                ),
            );
        }
    }

    fn send(
        &mut self,
        conversation: ConversationId,
        text: String,
        reply_to: Option<String>,
        mentions: Vec<MentionRef>,
        files: Vec<PathBuf>,
        images: Vec<crate::clipboard::InlineImage>,
    ) {
        let text = text.trim_end().to_owned();
        if text.trim().is_empty() && files.is_empty() && images.is_empty() {
            return;
        }
        let (html, mentions) = html::compose_with_mentions(&text, &mentions);
        let mut preview_html = html.clone();
        for image in &images {
            preview_html.push_str(&format!(
                "<img src=\"{}\" width=\"{}\" height=\"{}\">",
                image.uri(),
                image.width,
                image.height
            ));
            self.media.insert(image.uri(), Image::Ready(image.uri()));
        }
        self.next_local += 1;
        let local_id = format!("local-{}", self.next_local);
        let me = self.me().to_owned();
        let author = self
            .profile
            .as_ref()
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| "You".into());
        let channel_reply = conversation.is_channel() && reply_to.is_some();
        let demo = self.demo;
        let mut message = Message {
            id: local_id.clone(),
            conversation: Some(conversation.clone()),
            reply_to: if channel_reply {
                reply_to.clone()
            } else {
                None
            },
            author_id: Some(me.clone()),
            author,
            created: crate::time::now(),
            modified: crate::time::now(),
            body: html::parse(&preview_html),
            delivery: if demo {
                Delivery::Sent
            } else {
                Delivery::Sending
            },
            ..Default::default()
        };
        for path in &files {
            message.attachments.push(crate::model::Attachment {
                id: path.display().to_string(),
                kind: crate::model::AttachmentKind::File,
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
                content_type: Some("reference".into()),
                url: None,
            });
        }
        if let (Some(quoted), false) = (&reply_to, conversation.is_channel())
            && let Some(original) = self
                .conversations
                .get(&conversation)
                .and_then(|c| c.find(quoted))
        {
            message.attachments.push(crate::model::Attachment {
                id: quoted.clone(),
                kind: crate::model::AttachmentKind::Quote {
                    message_id: quoted.clone(),
                    author: Some(original.author.clone()),
                    preview: original.body.summary(200),
                },
                name: None,
                content_type: Some("messageReference".into()),
                url: None,
            });
        }
        if demo {
            message.id = format!("demo-{}", self.next_local);
        }
        // Who can open the files: everyone in the chat. Channel uploads get
        // an organisation link instead (Graph does not list channel members
        // with Chaat's permissions).
        let recipients: Vec<String> = match &conversation {
            ConversationId::Chat(chat) => self
                .chat(chat)
                .map(|c| {
                    c.members
                        .iter()
                        .filter(|m| m.user_id != me)
                        .filter_map(|m| m.email.clone())
                        .collect()
                })
                .unwrap_or_default(),
            ConversationId::Channel { .. } => Vec::new(),
        };
        let entry = self.conversations.entry(conversation.clone()).or_default();
        if channel_reply {
            entry.layout.invalidate();
            if let Some(root) = &reply_to {
                entry.heights.remove(root);
            }
            if let Some(root) = entry
                .messages
                .iter_mut()
                .find(|m| Some(&m.id) == reply_to.as_ref())
            {
                root.replies.push(message.clone());
            }
            entry.expanded.insert(reply_to.clone().unwrap_or_default());
        } else {
            entry.messages.push(message.clone());
        }
        if let ConversationId::Chat(chat_id) = &conversation
            && let Some(chat) = self.chats.iter_mut().find(|c| &c.id == chat_id)
        {
            chat.preview = Some(crate::model::Preview {
                message_id: message.id.clone(),
                author_id: Some(me),
                author: Some(message.author.clone()),
                text: preview_text(&message),
                at: message.created,
                deleted: false,
                ..Default::default()
            });
            self.chats.sort_by_key(Chat::list_order);
        }
        self.drafts.remove(&conversation);
        if !channel_reply {
            self.scroll_to_end = 3;
        }
        if !demo {
            self.pending_sends.insert(
                local_id.clone(),
                PendingSend {
                    html: html.clone(),
                    reply_to: reply_to.clone(),
                    mentions: mentions.clone(),
                    files: files.clone(),
                    images: images.clone(),
                    recipients: recipients.clone(),
                },
            );
            self.backend.send(Command::Send {
                conversation,
                local_id,
                html,
                reply_to,
                mentions,
                files,
                images,
                recipients,
            });
        }
    }

    fn create_chat(&mut self, members: Vec<Member>, topic: Option<String>) {
        if members.is_empty() {
            return;
        }
        self.overlay = None;
        let me = self.me().to_owned();
        // A one-on-one chat that exists already just opens.
        if members.len() == 1
            && let Some(existing) = self.chats.iter().find(|chat| {
                chat.kind == ChatKind::OneOnOne
                    && chat
                        .peer(&me)
                        .is_some_and(|p| p.user_id == members[0].user_id)
            })
        {
            let id = existing.id.clone();
            self.open_conversation(ConversationId::Chat(id));
            return;
        }
        if self.demo {
            let chat = crate::demo::new_chat(self, members, topic);
            let id = chat.id.clone();
            self.chats.insert(0, chat);
            self.conversations
                .entry(ConversationId::Chat(id.clone()))
                .or_default();
            self.open_conversation(ConversationId::Chat(id));
            return;
        }
        self.backend.send(Command::CreateChat {
            others: members.into_iter().map(|m| m.user_id).collect(),
            topic,
        });
        self.toast(ToastKind::Info, "Starting the chat…");
    }

    fn sign_out(&mut self) {
        self.overlay = None;
        if self.demo {
            return;
        }
        // Signing out of Chaat also ends plugin sign-ins for that account.
        if let Some(id) = self.id.clone() {
            self.forget_account_in_plugins(&id);
        }
        self.sign_out_browser();
        self.backend.send(Command::SignOut);
        let index = self.active;
        self.retire_slot(index);
        if self.accounts.is_empty() {
            self.add_slot(None);
            self.active = 0;
        }
        self.remember_accounts();
        self.fetch_view(Trigger::Focused);
    }

    /// Search over loaded messages, for the demo.
    fn local_search(&self, query: &str) -> Vec<SearchHit> {
        let needle = query.to_lowercase();
        let mut hits: Vec<SearchHit> = self
            .conversations
            .iter()
            .flat_map(|(id, conversation)| {
                conversation
                    .messages
                    .iter()
                    .flat_map(|m| std::iter::once(m).chain(m.replies.iter()))
                    .filter(|m| !m.system && m.body.to_plain().to_lowercase().contains(&needle))
                    .map(move |m| SearchHit {
                        conversation: id.clone(),
                        message_id: m
                            .reply_to
                            .clone()
                            .filter(|_| id.is_channel())
                            .unwrap_or_else(|| m.id.clone()),
                        author: m.author.clone(),
                        snippet: m.body.summary(140),
                        at: m.created,
                    })
            })
            .collect();
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.at));
        hits.truncate(50);
        hits
    }

    pub fn shutdown(&mut self) {
        self.save_settings();
        // Every account winds down at once; then wait for all of them.
        for account in &self.accounts {
            account.backend.begin_shutdown();
        }
        for account in &mut self.accounts {
            account.backend.shutdown();
        }
    }
}

/// What the chat list shows for a message: its text, or its files.
fn preview_text(message: &Message) -> String {
    message.preview_text()
}

impl fastframe_shell::Resident for App {
    fn closed(&self) -> fastframe_shell::Closed {
        let tray_shown = self
            .tray
            .as_ref()
            .is_some_and(fastframe_tray::Tray::is_shown);
        if self.quit_requested || !self.settings.keep_running || !tray_shown || self.demo {
            fastframe_shell::Closed::Quit
        } else {
            fastframe_shell::Closed::Hide
        }
    }

    fn window_gone(&mut self) {
        self.window_hidden = true;
        self.window_focused = false;
        self.wants_show = false;
        self.applied_palette = None;
    }

    fn headless_frame(&mut self, ctx: &egui::Context) -> fastframe_shell::Headless {
        self.background(ctx);
        if self.quit_requested {
            fastframe_shell::Headless::Quit
        } else if self.wants_show {
            self.window_hidden = false;
            fastframe_shell::Headless::Show
        } else {
            fastframe_shell::Headless::Wait
        }
    }

    fn shutdown(&mut self) {
        App::shutdown(self);
    }
}

fn toggle_reaction(reactions: &mut Vec<Reaction>, emoji: &str, me: &str, on: bool) {
    // Teams keeps one reaction per person per emoji.
    match reactions.iter_mut().find(|r| r.emoji == emoji) {
        Some(reaction) => {
            reaction.users.retain(|u| u != me);
            if on {
                reaction.users.push(me.to_owned());
            }
        }
        None if on => reactions.push(Reaction {
            emoji: emoji.to_owned(),
            users: vec![me.to_owned()],
        }),
        None => {}
    }
    reactions.retain(|r| !r.users.is_empty());
}

fn open_external(url: &str) {
    if let Err(error) = open::that_detached(url) {
        log::warn!("could not open a link: {error}");
    }
}

/// Hands image bytes to egui under a URI with the right extension, so its
/// loaders decode them. Unknown formats are refused.
fn register_image(ctx: &egui::Context, key: &str, bytes: Vec<u8>) -> Option<String> {
    let extension = match bytes.get(..4)? {
        [0xFF, 0xD8, ..] => "jpg",
        [0x89, b'P', b'N', b'G'] => "png",
        [b'G', b'I', b'F', _] => "gif",
        _ => return None,
    };
    let uri = format!("bytes://chaat/{key}.{extension}");
    crate::images::include(ctx, uri.clone(), bytes);
    Some(uri)
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::RichText;

    #[test]
    fn inactive_conversations_are_bounded_but_unsent_messages_and_drafts_survive() {
        let (mut app, mut commands) = app();
        let ctx = egui::Context::default();
        let unsent = ConversationId::Chat("unsent".into());
        let mut pending = message("pending", 1);
        pending.delivery = Delivery::Failed;
        app.conversations
            .entry(unsent.clone())
            .or_default()
            .messages
            .push(pending);
        app.recent_conversations.push_back(unsent.clone());
        let thread = ConversationId::Channel {
            team: "team".into(),
            channel: "channel".into(),
        };
        let mut root = message("root", 1);
        let mut reply = message("reply", 2);
        reply.delivery = Delivery::Sending;
        root.replies.push(reply);
        app.conversations
            .entry(thread.clone())
            .or_default()
            .messages
            .push(root);
        app.recent_conversations.push_back(thread.clone());
        let draft_id = ConversationId::Chat("draft".into());
        app.drafts.entry(draft_id.clone()).or_default().text = "Keep this".into();
        for i in 0..30 {
            let id = ConversationId::Chat(format!("chat-{i}"));
            app.open_conversation(id.clone());
            app.on_messages(
                id.clone(),
                vec![message("one", i)],
                Merge::Latest,
                Some(true),
                true,
            );
        }
        assert!(app.conversations.len() <= 12);
        assert!(app.conversations.contains_key(&unsent));
        assert!(app.conversations.contains_key(&thread));
        assert_eq!(app.drafts[&draft_id].text, "Keep this");
        let old = ConversationId::Chat("chat-0".into());
        assert!(!app.conversations.contains_key(&old));
        while commands.try_recv().is_ok() {}
        app.apply(Action::OpenConversation(old.clone()), &ctx);
        assert!(matches!(commands.try_recv().unwrap(), Command::CachedMessages(id) if id == old));
    }

    #[test]
    fn inactive_history_budget_counts_channel_replies() {
        let (mut app, _) = app();
        let heavy = ConversationId::Channel {
            team: "team".into(),
            channel: "channel".into(),
        };
        let mut root = message("root", 1);
        root.replies = (0..2_001).map(|i| message(&i.to_string(), i)).collect();
        app.conversations
            .entry(heavy.clone())
            .or_default()
            .messages
            .push(root);
        app.recent_conversations.push_front(heavy.clone());
        let active = ConversationId::Chat("active".into());
        app.open = Some(active.clone());
        app.conversations.entry(active.clone()).or_default();
        app.trim_conversations();
        assert!(!app.conversations.contains_key(&heavy));
        assert!(app.conversations.contains_key(&active));
    }

    #[test]
    fn a_late_disk_reopen_does_not_replace_fresh_messages() {
        let (mut app, _) = app();
        let chat = ConversationId::Chat("chat".into());
        let old = message("one", 1);
        let mut fresh = old.clone();
        fresh.body = RichText::plain("Edited on Microsoft Graph");
        app.on_messages(chat.clone(), vec![fresh], Merge::Latest, Some(false), false);
        app.on_messages(chat.clone(), vec![old], Merge::Latest, Some(true), true);
        let view = app.conversation(&chat).unwrap();
        assert_eq!(
            view.messages[0].body.to_plain(),
            "Edited on Microsoft Graph"
        );
        assert!(!view.more);
    }

    fn message(id: &str, created: i64) -> Message {
        Message {
            id: id.into(),
            created,
            body: RichText::plain(id),
            ..Default::default()
        }
    }

    #[test]
    fn names_graph_left_out_are_looked_up_once() {
        let (mut app, mut commands) = app();
        let ctx = egui::Context::default();
        while commands.try_recv().is_ok() {}
        app.apply(Action::LookUpNames(vec!["u-gone".into()]), &ctx);
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::UserName(id) if id == "u-gone"
        ));
        assert!(!app.needs_name("u-gone"), "asked already");
        app.apply(Action::LookUpNames(vec!["u-gone".into()]), &ctx);
        assert!(commands.try_recv().is_err(), "not asked twice");
        app.handle(
            Event::UserName {
                user: "u-gone".into(),
                name: Some("Sourabh".into()),
            },
            &ctx,
        );
        assert_eq!(app.person_name("u-gone").as_deref(), Some("Sourabh"));
        assert_eq!(app.person_name(app.me()).as_deref(), Some("You"));
    }

    #[test]
    fn retrying_the_relay_reconnects_only_a_failed_connection() {
        let (mut app, mut commands) = app();
        let ctx = egui::Context::default();
        while commands.try_recv().is_ok() {}
        app.settings.relay_url = Some("https://relay.example.com".into());
        app.push = crate::realtime::PushStatus::Live;
        app.apply(Action::RetryRelay, &ctx);
        assert!(commands.try_recv().is_err(), "a live connection stays");
        app.push = crate::realtime::PushStatus::Failed("timed out".into());
        app.apply(Action::RetryRelay, &ctx);
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Relay(Some(url)) if url == "https://relay.example.com"
        ));
    }

    #[test]
    fn the_last_read_divider_skips_own_and_system_messages() {
        let from = |id: &str, at, author: &str, system| Message {
            author_id: Some(author.into()),
            system,
            ..message(id, at)
        };
        let mut view = Conversation {
            messages: vec![
                from("a", 10, "u-ada", false),
                from("b", 20, "u-me", false),
                from("c", 30, "u-ada", true),
                from("d", 40, "u-ada", false),
            ],
            ..Default::default()
        };
        assert_eq!(view.place_divider("u-me"), None, "no divider by default");
        view.set_unread_after(Some(10));
        assert_eq!(view.place_divider("u-me"), Some(3));
        view.set_unread_after(Some(40));
        assert_eq!(view.place_divider("u-me"), None, "nothing newer");
    }

    #[test]
    fn a_moving_divider_measures_its_old_and_new_rows_again() {
        let from = |id: &str, at| Message {
            author_id: Some("u-ada".into()),
            ..message(id, at)
        };
        // More was unread than one page: the divider starts at the oldest
        // loaded message.
        let mut view = Conversation {
            messages: vec![from("c", 30), from("d", 40)],
            unread_after: Some(5),
            ..Default::default()
        };
        assert_eq!(view.place_divider("u-me"), Some(0));
        for id in ["c", "d"] {
            view.heights.insert(id.into(), 50.0);
        }
        // Older history arrives and the divider moves up to it.
        view.messages.splice(0..0, [from("a", 10), from("b", 20)]);
        view.heights.insert("a".into(), 50.0);
        assert_eq!(view.place_divider("u-me"), Some(0));
        assert!(
            !view.heights.contains_key("c"),
            "the old row loses the divider"
        );
        assert!(!view.heights.contains_key("a"), "the new row gains it");
        assert!(
            view.heights.contains_key("d"),
            "other rows keep their height"
        );
    }

    fn app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
        let (backend, commands) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-unused");
        let mut app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            false,
        );
        app.auth = AuthStatus::SignedIn {
            account: "u-me.t1".into(),
            name: "Me".into(),
            username: "me@contoso.com".into(),
        };
        (app, commands)
    }

    fn ring(id: &str, caller: &str) -> crate::calling::RingAlert {
        crate::calling::RingAlert {
            id: id.into(),
            caller: "Caller".into(),
            caller_object_id: Some(caller.to_owned()),
            video: false,
            group: false,
            thread_id: None,
            account: None,
            username: None,
            ring_timeout: None,
        }
    }

    fn ada_chat(app: &App) -> ConversationId {
        let me = app.me().to_owned();
        let chat = app
            .chats
            .iter()
            .find(|c| {
                c.kind == ChatKind::OneOnOne && c.peer(&me).is_some_and(|p| p.user_id == "u-ada")
            })
            .expect("the demo has a chat with Ada");
        ConversationId::Chat(chat.id.clone())
    }

    #[test]
    fn a_ring_resolves_the_caller_from_their_object_id_and_raises_the_window() {
        let (mut app, _dir) = demo_app();
        app.wants_show = false;
        app.on_incoming_call(ring("call-1", "u-ada"));
        let shown = &app.rings[0];
        assert_eq!(shown.chat, Some(ada_chat(&app)));
        assert!(shown.email().is_some_and(|e| e.contains('@')));
        assert!(app.wants_show);
        // The same call again replaces the toast rather than stacking it.
        app.on_incoming_call(ring("call-1", "u-ada"));
        assert_eq!(app.rings.len(), 1);
    }

    #[test]
    fn open_chat_from_a_ring_opens_the_callers_chat_and_closes_the_toast() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        app.section = Section::Calls;
        app.on_incoming_call(ring("call-1", "u-ada"));
        app.apply(Action::RingOpenChat("call-1".into()), &ctx);
        assert_eq!(app.open, Some(ada_chat(&app)));
        assert_eq!(app.section, Section::Chats);
        assert!(app.rings.is_empty());
    }

    #[test]
    fn an_unknown_caller_cannot_open_a_chat_and_a_call_back_falls_back_to_teams() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        app.on_incoming_call(ring("call-2", "someone-new"));
        let shown = &app.rings[0];
        assert!(shown.member.is_none() && shown.chat.is_none());
        assert!(!shown.can_open_chat());
        app.apply(Action::RingPlaceCall("call-2".into()), &ctx);
        assert!(app.rings.is_empty());
        assert!(app.call.is_none());
        assert!(app.toasts.iter().any(|t| t.text.contains("opened Teams")));
    }

    fn plugin_event(app: &App, id: &str, event: crate::plugin::event::PluginEvent) -> Event {
        Event::Plugin {
            id: id.into(),
            generation: app.plugin_generations.get(id).copied().unwrap_or(0),
            event,
        }
    }

    fn sign_in_event(account: Option<&str>, code: &str) -> crate::plugin::event::PluginEvent {
        crate::plugin::event::PluginEvent::SignIn {
            account: account.map(str::to_owned),
            verification_uri: "https://microsoft.com/devicelogin".into(),
            user_code: code.into(),
        }
    }

    fn signed_in_event(account: Option<&str>) -> crate::plugin::event::PluginEvent {
        crate::plugin::event::PluginEvent::SignedIn {
            account: account.map(str::to_owned),
        }
    }

    fn ready_event(
        needs: &[&'static str],
        offers: &[&'static str],
        options: Vec<crate::plugin::protocol::OptionSpec>,
        forgotten: &[&str],
    ) -> crate::plugin::event::PluginEvent {
        crate::plugin::event::PluginEvent::Ready {
            info: Default::default(),
            offers: offers.to_vec(),
            needs: needs.to_vec(),
            options,
            forgotten: forgotten.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    /// An enabled entry in the plugin list, not started, that the person
    /// already allowed everything to.
    fn plugin_entry(app: &mut App, id: &str, needs: &[&str]) {
        app.settings.plugins.push(crate::settings::PluginEntry {
            id: id.into(),
            path: PathBuf::from(format!("{id}.exe")),
            enabled: true,
            needs: needs.iter().map(|n| (*n).to_owned()).collect(),
            approved_needs: vec!["accounts".into(), "presence".into()],
            instance: format!("inst-{id}"),
            ..Default::default()
        });
    }

    /// A running call-alerts plugin, as after its `ready`.
    fn call_alerts_plugin(app: &mut App, id: &str) {
        plugin_entry(app, id, &["accounts"]);
        app.plugins.insert(
            id.into(),
            PluginState {
                running: true,
                offers: vec![crate::plugin::registry::CALL_ALERTS],
                needs: vec!["accounts"],
                ..Default::default()
            },
        );
    }

    fn sent(commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| commands.try_recv().ok()).collect()
    }

    fn busy() -> Presence {
        Presence {
            availability: crate::model::Availability::Busy,
            ..Default::default()
        }
    }

    fn app_with_service() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
        let (mut app, _) = app();
        let (service, commands) = Backend::recording();
        app.service = service;
        app.id = Some("u-me.t1".into());
        (app, commands)
    }

    #[test]
    fn the_plugin_sign_in_panel_shows_until_signed_in_or_stopped() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        call_alerts_plugin(&mut app, "ca");
        app.handle(plugin_event(&app, "ca", sign_in_event(None, "AB12")), &ctx);
        assert_eq!(app.plugins["ca"].sign_in[0].user_code, "AB12");
        app.handle(plugin_event(&app, "ca", signed_in_event(None)), &ctx);
        assert!(app.plugins["ca"].sign_in.is_empty());
        app.handle(plugin_event(&app, "ca", sign_in_event(None, "AB12")), &ctx);
        app.handle(
            plugin_event(
                &app,
                "ca",
                crate::plugin::event::PluginEvent::Status {
                    running: false,
                    message: None,
                },
            ),
            &ctx,
        );
        assert!(app.plugins["ca"].sign_in.is_empty());
        app.handle(plugin_event(&app, "ca", sign_in_event(None, "AB12")), &ctx);
        app.apply(
            Action::HidePluginSignIn {
                plugin: "ca".into(),
                account: None,
            },
            &ctx,
        );
        assert!(app.plugins["ca"].sign_in.is_empty());
    }

    #[test]
    fn plugins_turned_on_start_once_the_service_is_attached() {
        // At launch App::new syncs before main attaches the service backend.
        let (mut app, _) = app();
        assert!(app.service.is_detached());
        app.settings.plugins.push(crate::settings::PluginEntry {
            id: "pr".into(),
            path: PathBuf::from("plugin.exe"),
            enabled: true,
            ..Default::default()
        });
        app.sync_plugins();
        let (service, mut commands) = Backend::recording();
        app.attach_service(service);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::StartPlugin { id, .. }) if id == "pr"
        ));
    }

    #[test]
    fn enabling_and_disabling_a_plugin_starts_and_stops_it() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        app.settings.plugins.push(crate::settings::PluginEntry {
            id: "ca".into(),
            path: PathBuf::from("plugin.exe"),
            enabled: false,
            needs: vec!["accounts".into()],
            approved_needs: vec!["accounts".into()],
            ..Default::default()
        });
        app.sync_plugins();
        assert!(commands.try_recv().is_err(), "off: nothing starts");
        app.apply(Action::SetPluginEnabled("ca".into(), true), &ctx);
        let me = crate::calling::PluginAccount {
            id: "u-me.t1".into(),
            username: "me@contoso.com".into(),
        };
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::StartPlugin { id, path, hello: crate::plugin::protocol::HostMessage::Hello { accounts, .. }, .. })
                if id == "ca" && path == std::path::Path::new("plugin.exe") && accounts == vec![me.clone()]
        ));
        // Syncing again with nothing changed starts nothing.
        app.sync_plugins();
        assert!(commands.try_recv().is_err());
        // A new path restarts that plugin only.
        app.settings.plugins[0].path = PathBuf::from("other.exe");
        app.sync_plugins();
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::StartPlugin { path, .. }) if path == std::path::Path::new("other.exe")
        ));
        app.apply(Action::SetPluginEnabled("ca".into(), false), &ctx);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::StopPlugin { id }) if id == "ca"
        ));
    }

    #[test]
    fn only_the_first_sign_in_code_raises_the_window() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        call_alerts_plugin(&mut app, "ca");
        app.wants_show = false;
        app.handle(plugin_event(&app, "ca", sign_in_event(None, "FIRST")), &ctx);
        assert!(app.wants_show);
        app.wants_show = false;
        app.apply(
            Action::HidePluginSignIn {
                plugin: "ca".into(),
                account: None,
            },
            &ctx,
        );
        app.handle(
            plugin_event(&app, "ca", sign_in_event(None, "SECOND")),
            &ctx,
        );
        assert!(!app.wants_show, "a later code doesn't pull Chaat forward");
        assert_eq!(app.plugins["ca"].sign_in[0].user_code, "SECOND");
        // After signing in, a future expiry may raise it again.
        app.handle(plugin_event(&app, "ca", signed_in_event(None)), &ctx);
        app.handle(plugin_event(&app, "ca", sign_in_event(None, "THIRD")), &ctx);
        assert!(app.wants_show);
    }

    #[test]
    fn accounts_go_only_to_plugins_that_need_them() {
        let (mut app, mut commands) = app_with_service();
        plugin_entry(&mut app, "ca", &["accounts"]);
        plugin_entry(&mut app, "light", &[]);
        app.sync_plugins();
        let started = sent(&mut commands);
        let accounts_in = |id: &str| {
            started.iter().find_map(|c| match c {
                Command::StartPlugin {
                    id: started,
                    hello: crate::plugin::protocol::HostMessage::Hello { accounts, .. },
                    ..
                } if started == id => Some(accounts.len()),
                _ => None,
            })
        };
        assert_eq!(accounts_in("ca"), Some(1));
        assert_eq!(accounts_in("light"), Some(0));
        // Nothing changed: nothing to tell.
        app.remember_accounts();
        assert!(commands.try_recv().is_err());
        app.auth = AuthStatus::SignedIn {
            account: "u-me.t1".into(),
            name: "Me".into(),
            username: "me@fabrikam.com".into(),
        };
        app.remember_accounts();
        let changed = sent(&mut commands);
        assert!(changed.iter().any(|c| matches!(
            c,
            Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Accounts { accounts } }
                if id == "ca" && accounts[0].username == "me@fabrikam.com"
        )));
        assert!(changed.iter().any(|c| matches!(
            c,
            Command::PluginHello { id, accounts: Some(_), .. } if id == "ca"
        )));
        assert!(
            !changed.iter().any(|c| matches!(
                c,
                Command::PluginSend { id, .. } | Command::PluginHello { id, .. } if id == "light"
            )),
            "a plugin that needs no accounts hears nothing about them"
        );
    }

    #[test]
    fn a_ready_that_asks_for_accounts_gets_them_once() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "new", &[]);
        app.sync_plugins();
        sent(&mut commands);
        app.handle(
            plugin_event(
                &app,
                "new",
                ready_event(&["accounts"], &[], Vec::new(), &[]),
            ),
            &ctx,
        );
        let after = sent(&mut commands);
        assert!(after.iter().any(|c| matches!(
            c,
            Command::PluginSend {
                message: crate::plugin::protocol::HostMessage::Accounts { .. },
                ..
            }
        )));
        assert_eq!(app.settings.plugins[0].needs, vec!["accounts".to_owned()]);
        app.handle(
            plugin_event(
                &app,
                "new",
                ready_event(&["accounts"], &[], Vec::new(), &[]),
            ),
            &ctx,
        );
        assert!(!sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::PluginSend {
                message: crate::plugin::protocol::HostMessage::Accounts { .. },
                ..
            }
        )));
    }

    #[test]
    fn presence_goes_only_to_plugins_that_need_it() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        app.my_presence = Some(busy());
        plugin_entry(&mut app, "light", &[]);
        plugin_entry(&mut app, "other", &[]);
        app.sync_plugins();
        sent(&mut commands);
        app.handle(
            plugin_event(
                &app,
                "light",
                ready_event(&["presence"], &["presence_display"], Vec::new(), &[]),
            ),
            &ctx,
        );
        app.handle(
            plugin_event(
                &app,
                "other",
                ready_event(&[], &["presence_display"], Vec::new(), &[]),
            ),
            &ctx,
        );
        let presences: Vec<Command> = sent(&mut commands)
            .into_iter()
            .filter(|c| {
                matches!(
                    c,
                    Command::PluginSend {
                        message: crate::plugin::protocol::HostMessage::Presence { .. },
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(presences.len(), 1);
        assert!(matches!(
            &presences[0],
            Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Presence { availability, activity } }
                if id == "light" && availability == "Busy" && activity.is_none()
        ));
    }

    #[test]
    fn presence_is_resent_only_when_it_changes() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        app.my_presence = Some(busy());
        plugin_entry(&mut app, "light", &[]);
        app.sync_plugins();
        app.handle(
            plugin_event(
                &app,
                "light",
                ready_event(&["presence"], &[], Vec::new(), &[]),
            ),
            &ctx,
        );
        let count = |commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>| {
            sent(commands)
                .iter()
                .filter(|c| {
                    matches!(
                        c,
                        Command::PluginSend {
                            message: crate::plugin::protocol::HostMessage::Presence { .. },
                            ..
                        }
                    )
                })
                .count()
        };
        assert_eq!(count(&mut commands), 1);
        app.sync_plugin_presence();
        app.handle(Event::MyPresence(busy()), &ctx);
        assert_eq!(count(&mut commands), 0, "the same presence is not resent");
        app.handle(
            Event::MyPresence(Presence {
                availability: crate::model::Availability::Available,
                ..Default::default()
            }),
            &ctx,
        );
        assert_eq!(count(&mut commands), 1);
    }

    #[test]
    fn presence_is_resent_when_the_account_signs_out() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        app.my_presence = Some(busy());
        plugin_entry(&mut app, "light", &[]);
        app.sync_plugins();
        app.handle(
            plugin_event(
                &app,
                "light",
                ready_event(&["presence"], &[], Vec::new(), &[]),
            ),
            &ctx,
        );
        sent(&mut commands);
        app.handle(Event::Auth(AuthStatus::SignedOut), &ctx);
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::PluginSend {
                message: crate::plugin::protocol::HostMessage::Presence { availability, .. },
                ..
            } if availability == "Unknown"
        )));
    }

    #[test]
    fn a_plugin_that_drops_and_regains_the_accounts_need_is_sent_them_again() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "p", &["accounts"]);
        app.sync_plugins();
        app.handle(
            plugin_event(&app, "p", ready_event(&[], &[], Vec::new(), &[])),
            &ctx,
        );
        assert!(app.plugins["p"].sent_accounts.is_none());
        sent(&mut commands);
        app.handle(
            plugin_event(&app, "p", ready_event(&["accounts"], &[], Vec::new(), &[])),
            &ctx,
        );
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::PluginSend {
                message: crate::plugin::protocol::HostMessage::Accounts { .. },
                ..
            }
        )));
    }

    #[test]
    fn a_second_call_alerts_plugin_is_ignored() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        call_alerts_plugin(&mut app, "first");
        call_alerts_plugin(&mut app, "second");
        assert_eq!(app.call_alerts_owner().as_deref(), Some("first"));
        app.handle(
            plugin_event(
                &app,
                "second",
                crate::plugin::event::PluginEvent::IncomingCall(ring("c1", "u-ada")),
            ),
            &ctx,
        );
        app.handle(
            plugin_event(&app, "second", sign_in_event(None, "NO")),
            &ctx,
        );
        assert!(app.rings.is_empty());
        assert!(app.plugins["second"].sign_in.is_empty());
        app.handle(
            plugin_event(
                &app,
                "first",
                crate::plugin::event::PluginEvent::IncomingCall(ring("c1", "u-ada")),
            ),
            &ctx,
        );
        assert_eq!(app.rings.len(), 1);
        // With the first one off, the second takes over.
        app.settings.plugins[0].enabled = false;
        assert_eq!(app.call_alerts_owner().as_deref(), Some("second"));
    }

    #[test]
    fn a_non_owner_call_alerts_plugin_shows_no_sign_in_notices() {
        use crate::plugin::event::PluginEvent;
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        call_alerts_plugin(&mut app, "first");
        call_alerts_plugin(&mut app, "second");
        let toasts = app.toasts.len();
        let account = || Some("u-me.demo".to_owned());
        app.handle(
            plugin_event(&app, "second", PluginEvent::SignedIn { account: account() }),
            &ctx,
        );
        app.handle(
            plugin_event(
                &app,
                "second",
                PluginEvent::Listening {
                    account: "u-me.demo".into(),
                },
            ),
            &ctx,
        );
        app.handle(
            plugin_event(
                &app,
                "second",
                PluginEvent::Error {
                    account: account(),
                    message: "nope".into(),
                },
            ),
            &ctx,
        );
        assert_eq!(app.toasts.len(), toasts);
        assert!(app.plugins["second"].listening.is_empty());
        // The owner still gets its notices.
        app.handle(
            plugin_event(&app, "first", PluginEvent::SignedIn { account: account() }),
            &ctx,
        );
        assert_eq!(app.toasts.len(), toasts + 1);
    }

    #[test]
    fn sign_in_cards_key_on_plugin_and_account() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        for id in ["a", "b"] {
            plugin_entry(&mut app, id, &[]);
            app.plugins.insert(id.into(), PluginState::default());
        }
        app.handle(
            plugin_event(&app, "a", sign_in_event(Some("u-me.demo"), "A1")),
            &ctx,
        );
        app.handle(
            plugin_event(&app, "b", sign_in_event(Some("u-me.demo"), "B1")),
            &ctx,
        );
        app.handle(
            plugin_event(&app, "b", sign_in_event(Some("u-me.demo"), "B2")),
            &ctx,
        );
        assert_eq!(app.plugins["a"].sign_in.len(), 1);
        assert_eq!(app.plugins["b"].sign_in.len(), 1);
        assert_eq!(app.plugins["a"].sign_in[0].user_code, "A1");
        assert_eq!(app.plugins["b"].sign_in[0].user_code, "B2");
        assert_eq!(app.plugins["b"].sign_in[0].plugin, "b");
        app.apply(
            Action::HidePluginSignIn {
                plugin: "a".into(),
                account: Some("u-me.demo".into()),
            },
            &ctx,
        );
        assert!(app.plugins["a"].sign_in.is_empty());
        assert_eq!(app.plugins["b"].sign_in.len(), 1);
    }

    #[test]
    fn removing_a_plugin_deletes_its_secrets_and_stops_it() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "light", &["accounts"]);
        app.settings.plugins[0].secrets = vec!["token".into(), "key".into()];
        app.sync_plugins();
        sent(&mut commands);
        app.apply(Action::RemovePlugin("light".into()), &ctx);
        let after = sent(&mut commands);
        assert!(after.iter().any(|c| matches!(
            c,
            Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Forget { account } }
                if id == "light" && account == "u-me.t1"
        )));
        for option in ["token", "key"] {
            assert!(after.iter().any(|c| matches!(
                c,
                Command::SavePluginSecret { plugin, option: o, value: None, .. }
                    if plugin == "light" && o == option
            )));
        }
        assert!(
            after
                .iter()
                .any(|c| matches!(c, Command::StopPlugin { id } if id == "light"))
        );
        assert!(app.settings.plugins.is_empty());
        assert!(!app.plugins.contains_key("light"));
    }

    #[test]
    fn adding_a_plugin_names_it_from_its_file_and_starts_it() {
        let (mut app, mut commands) = app_with_service();
        app.add_plugin(PathBuf::from("C:/x/Chaat Status-Light.exe"));
        app.add_plugin(PathBuf::from("D:/y/chaat status-light.exe"));
        let ids: Vec<&str> = app.settings.plugins.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["chaat-status-light", "chaat-status-light-2"]);
        assert!(app.settings.plugins.iter().all(|e| e.enabled));
        let started = sent(&mut commands);
        assert_eq!(
            started
                .iter()
                .filter(|c| matches!(c, Command::StartPlugin { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn options_are_sent_after_ready_and_on_change() {
        use crate::plugin::protocol::{OptionKind, OptionSpec};
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "light", &[]);
        app.settings.plugins[0]
            .options
            .insert("ip".into(), serde_json::json!("10.0.0.5"));
        app.settings.plugins[0].secrets = vec!["key".into()];
        app.sync_plugins();
        sent(&mut commands);
        let spec = |id: &str, kind, default| OptionSpec {
            id: id.into(),
            kind,
            label: id.into(),
            default,
            choices: Vec::new(),
            ..Default::default()
        };
        let options = vec![
            spec("ip", OptionKind::Text, Some(serde_json::json!(""))),
            spec("flash", OptionKind::Toggle, Some(serde_json::json!(true))),
            spec("key", OptionKind::Secret, None),
        ];
        app.handle(
            plugin_event(&app, "light", ready_event(&[], &[], options, &[])),
            &ctx,
        );
        let values = |commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>| {
            sent(commands).into_iter().find_map(|c| match c {
                Command::PluginOptions {
                    id,
                    values,
                    secrets,
                } if id == "light" => Some((values, secrets)),
                _ => None,
            })
        };
        let (first, secrets) = values(&mut commands).expect("options after ready");
        assert_eq!(first["ip"], serde_json::json!("10.0.0.5"));
        assert_eq!(first["flash"], serde_json::json!(true));
        assert_eq!(secrets, vec!["key".to_owned()]);
        app.apply(
            Action::SetPluginOption {
                plugin: "light".into(),
                option: "flash".into(),
                value: serde_json::json!(false),
            },
            &ctx,
        );
        let (changed, _) = values(&mut commands).expect("options after a change");
        assert_eq!(changed["flash"], serde_json::json!(false));
        assert_eq!(
            app.settings.plugins[0].options["flash"],
            serde_json::json!(false)
        );
        // A cleared secret leaves the list, a stored one joins it, and the
        // plugin hears each time.
        let saved = |set: bool| Event::PluginSecretSaved {
            plugin: "light".into(),
            instance: "inst-light".into(),
            option: "key".into(),
            set,
            ok: true,
        };
        app.handle(saved(false), &ctx);
        let (_, secrets) = values(&mut commands).expect("options after a clear");
        assert!(secrets.is_empty());
        app.handle(saved(true), &ctx);
        let (_, secrets) = values(&mut commands).expect("options after a secret");
        assert_eq!(secrets, vec!["key".to_owned()]);
    }

    #[test]
    fn calling_back_a_group_ring_follows_the_callers_own_chat() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        let mut alert = ring("group-1", "u-ada");
        alert.group = true;
        alert.thread_id = Some(LAUNCH.into());
        app.on_incoming_call(alert);
        assert_eq!(app.rings[0].chat, Some(ConversationId::Chat(LAUNCH.into())));
        app.apply(Action::RingPlaceCall("group-1".into()), &ctx);
        let call = app.call.as_ref().expect("the call back starts");
        assert_eq!(call.chat, Some(ada_chat(&app)));
    }

    const FABRIKAM: &str = "u-alex2.fabrikam";
    const FABRIKAM_MAE: &str = "19:fabrikam-mae@unq.gbl.spaces";

    fn fabrikam_index(app: &App) -> usize {
        app.accounts
            .iter()
            .position(|a| a.id.as_deref() == Some(FABRIKAM))
            .expect("the demo has a Fabrikam account")
    }

    #[test]
    fn a_ring_on_another_account_resolves_there_and_switches_to_it() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        let fabrikam = fabrikam_index(&app);
        assert_ne!(app.active, fabrikam);
        let mut alert = ring("f-call", "u-mae");
        alert.account = Some(FABRIKAM.into());
        app.on_incoming_call(alert);
        let shown = &app.rings[0];
        assert_eq!(shown.account.as_deref(), Some(FABRIKAM));
        assert_eq!(shown.account_name.as_deref(), Some("alex@fabrikam.com"));
        assert_eq!(shown.chat, Some(ConversationId::Chat(FABRIKAM_MAE.into())));
        assert_eq!(shown.email(), Some("mae@fabrikam.com"));
        let key = app.rings[0].id.clone();
        app.apply(Action::RingOpenChat(key), &ctx);
        assert_eq!(app.active, fabrikam);
        assert_eq!(app.open, Some(ConversationId::Chat(FABRIKAM_MAE.into())));
    }

    #[test]
    fn a_ring_on_an_account_chaat_lacks_shows_unresolved() {
        let (mut app, _dir) = demo_app();
        let active = app.active;
        let mut alert = ring("x-call", "u-ada");
        alert.account = Some("other.tenant".into());
        alert.username = Some("someone@elsewhere.com".into());
        app.on_incoming_call(alert);
        let shown = &app.rings[0];
        assert_eq!(shown.account, None);
        assert_eq!(shown.account_name.as_deref(), Some("someone@elsewhere.com"));
        assert!(shown.member.is_none() && !shown.can_open_chat());
        let ctx = egui::Context::default();
        let key = app.rings[0].id.clone();
        app.apply(Action::RingPlaceCall(key), &ctx);
        assert_eq!(app.active, active, "no account to switch to");
    }

    #[test]
    fn each_account_gets_its_own_sign_in_card() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        call_alerts_plugin(&mut app, "ca");
        let ask = |app: &App, account: &str, code: &str| {
            plugin_event(app, "ca", sign_in_event(Some(account), code))
        };
        app.handle(ask(&app, "u-me.demo", "ONE"), &ctx);
        app.handle(ask(&app, FABRIKAM, "TWO"), &ctx);
        app.handle(ask(&app, FABRIKAM, "THREE"), &ctx);
        assert_eq!(app.plugins["ca"].sign_in.len(), 2);
        let fabrikam = app.plugins["ca"]
            .sign_in
            .iter()
            .find(|s| s.account.as_deref() == Some(FABRIKAM))
            .unwrap();
        assert_eq!(fabrikam.user_code, "THREE");
        assert_eq!(fabrikam.username.as_deref(), Some("alex@fabrikam.com"));
        app.handle(
            plugin_event(&app, "ca", signed_in_event(Some(FABRIKAM))),
            &ctx,
        );
        assert_eq!(app.plugins["ca"].sign_in.len(), 1);
        app.handle(
            plugin_event(
                &app,
                "ca",
                crate::plugin::event::PluginEvent::Listening {
                    account: FABRIKAM.into(),
                },
            ),
            &ctx,
        );
        assert_eq!(app.plugins["ca"].listening, vec![FABRIKAM.to_owned()]);
    }

    #[test]
    fn one_call_ringing_two_accounts_shows_a_card_for_each() {
        let (mut app, _dir) = demo_app();
        for account in ["u-me.demo", FABRIKAM] {
            let mut alert = ring("same-call", "u-ada");
            alert.account = Some(account.into());
            app.on_incoming_call(alert);
        }
        assert_eq!(app.rings.len(), 2);
    }

    #[test]
    fn an_account_error_names_the_account_and_stops_listening() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        call_alerts_plugin(&mut app, "ca");
        app.plugins.get_mut("ca").unwrap().info.name = Some("Call alerts".into());
        app.handle(
            plugin_event(
                &app,
                "ca",
                crate::plugin::event::PluginEvent::Listening {
                    account: FABRIKAM.into(),
                },
            ),
            &ctx,
        );
        app.handle(
            plugin_event(
                &app,
                "ca",
                crate::plugin::event::PluginEvent::Error {
                    account: Some(FABRIKAM.into()),
                    message: "could not find the push service host".into(),
                },
            ),
            &ctx,
        );
        assert!(app.plugins["ca"].listening.is_empty());
        assert!(app.toasts.iter().any(|t| {
            t.text
                .starts_with("Plugin \u{201c}Call alerts\u{201d} for alex@fabrikam.com:")
        }));
    }

    #[test]
    fn signing_out_with_a_plugin_off_still_clears_its_sign_in() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "ca", &["accounts"]);
        app.settings.plugins[0].enabled = false;
        app.apply(Action::SignOut, &ctx);
        // Not running: queued for the next start, which tells the plugin.
        assert_eq!(app.settings.plugins[0].forget, vec!["u-me.t1".to_owned()]);
        assert!(commands.try_recv().is_err());
        app.settings.plugins[0].enabled = true;
        app.sync_plugins();
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::StartPlugin { hello: crate::plugin::protocol::HostMessage::Hello { forget, .. }, .. })
                if forget == vec!["u-me.t1".to_owned()]
        ));
        // Once the plugin is ready, the queue is done.
        app.handle(
            plugin_event(
                &app,
                "ca",
                ready_event(&["accounts"], &[], Vec::new(), &["u-me.t1"]),
            ),
            &ctx,
        );
        assert!(app.settings.plugins[0].forget.is_empty());
        assert!(app.plugins["ca"].running);
    }

    #[test]
    fn a_sign_out_queues_forget_per_plugin() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "a", &["accounts"]);
        plugin_entry(&mut app, "b", &["accounts"]);
        plugin_entry(&mut app, "c", &[]);
        app.sync_plugins();
        sent(&mut commands);
        app.apply(Action::SignOut, &ctx);
        let queue = |id: &str| {
            app.settings
                .plugins
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .forget
                .clone()
        };
        assert_eq!(queue("a"), vec!["u-me.t1".to_owned()]);
        assert_eq!(queue("b"), vec!["u-me.t1".to_owned()]);
        assert!(queue("c").is_empty());
        let after = sent(&mut commands);
        for id in ["a", "b"] {
            assert!(after.iter().any(|c| matches!(
                c,
                Command::PluginSend { id: to, message: crate::plugin::protocol::HostMessage::Forget { account } }
                    if to == id && account == "u-me.t1"
            )));
        }
        assert!(
            !after.iter().any(|c| matches!(
                c,
                Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Forget { .. } } if id == "c"
            )),
            "a plugin that needs no accounts is not told to forget one"
        );
    }

    #[test]
    fn removing_an_account_drops_its_plugin_state() {
        let (mut app, _commands) = app_with_service();
        plugin_entry(&mut app, "ca", &["accounts"]);
        app.plugins.insert(
            "ca".into(),
            PluginState {
                running: true,
                listening: vec!["u-me.t1".into(), "gone.t9".into()],
                sign_in: vec![crate::model::PluginSignIn {
                    plugin: "ca".into(),
                    account: Some("gone.t9".into()),
                    username: None,
                    verification_uri: "https://microsoft.com/devicelogin".into(),
                    user_code: "OLD".into(),
                }],
                ..Default::default()
            },
        );
        app.remember_accounts();
        assert_eq!(app.plugins["ca"].listening, vec!["u-me.t1".to_owned()]);
        assert!(app.plugins["ca"].sign_in.is_empty());
    }

    #[test]
    fn a_ring_card_stays_as_long_as_the_plugin_says_the_call_rings() {
        let (mut app, _dir) = demo_app();
        let mut alert = ring("short", "u-ada");
        alert.ring_timeout = Some(std::time::Duration::from_secs(15));
        app.on_incoming_call(alert);
        app.on_incoming_call(ring("default", "u-ada"));
        assert_eq!(app.rings[0].shown_for, std::time::Duration::from_secs(15));
        assert_eq!(app.rings[1].shown_for, crate::calling::alerts::DEFAULT_RING);
    }

    #[test]
    fn a_sign_out_stays_queued_until_the_plugin_confirms_or_the_account_returns() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "ca", &["accounts"]);
        app.sync_plugins();
        sent(&mut commands);
        app.apply(Action::SignOut, &ctx);
        let after = sent(&mut commands);
        // Told at once, and still queued in case it never arrives.
        assert!(after.iter().any(|c| matches!(
            c,
            Command::PluginSend { id, message: crate::plugin::protocol::HostMessage::Forget { account } }
                if id == "ca" && account == "u-me.t1"
        )));
        assert_eq!(app.settings.plugins[0].forget, vec!["u-me.t1".to_owned()]);
        // A ready that didn't clear it keeps it queued.
        app.handle(
            plugin_event(&app, "ca", ready_event(&["accounts"], &[], Vec::new(), &[])),
            &ctx,
        );
        assert_eq!(app.settings.plugins[0].forget, vec!["u-me.t1".to_owned()]);
        sent(&mut commands);
        // Signing that account in again takes it off the queue.
        app.id = Some("u-me.t1".into());
        app.remember_accounts();
        assert!(app.settings.plugins[0].forget.is_empty());
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::PluginHello { id, forget: Some(forget), .. } if id == "ca" && forget.is_empty()
        )));
    }
    #[test]
    fn dismissing_a_ring_removes_only_that_one() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        app.on_incoming_call(ring("a", "u-ada"));
        app.on_incoming_call(ring("b", "u-grace"));
        app.apply(Action::DismissRing("a".into()), &ctx);
        assert_eq!(app.rings.len(), 1);
        assert_eq!(app.rings[0].id, "b");
    }
    const LAUNCH: &str = "19:demo-launch@thread.v2";

    fn demo_app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(
            AppDirs::under(dir.path()),
            Settings::default(),
            Waker::default(),
            true,
        );
        // The demo's own plugins would own call alerts; tests set up theirs.
        app.settings.plugins.clear();
        app.plugins.clear();
        (app, dir)
    }

    fn launch_members(app: &App) -> Vec<String> {
        app.group_members(LAUNCH)
            .into_iter()
            .map(|m| m.user_id)
            .collect()
    }

    fn person(id: &str) -> Member {
        Member {
            user_id: id.into(),
            display_name: id.into(),
            ..Default::default()
        }
    }

    /// A signed-in, non-demo app with one group chat of me and Ada.
    fn group_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
        let (mut app, commands) = app();
        app.granted = "User.Read Chat.ReadWrite ChatMember.ReadWrite".into();
        app.chats = vec![Chat {
            id: "19:g".into(),
            kind: ChatKind::Group,
            members: vec![person("u-me"), person("u-ada")],
            ..Default::default()
        }];
        (app, commands)
    }

    #[test]
    fn group_members_lists_me_first_then_by_name() {
        let (app, _dir) = demo_app();
        assert_eq!(
            launch_members(&app),
            ["u-me", "u-ada", "u-grace", "u-katherine", "u-margaret"]
        );
        assert!(app.group_members("unknown").is_empty());
    }

    #[test]
    fn open_add_people_resets_scratch() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        app.add_people.picked.push(person("u-alan"));
        app.add_people.history_step = true;
        app.apply(Action::OpenAddPeople(LAUNCH.into()), &ctx);
        assert_eq!(app.overlay, Some(Overlay::AddPeople(LAUNCH.into())));
        assert!(app.add_people.picked.is_empty());
        assert!(!app.add_people.history_step);
    }

    #[test]
    fn add_members_in_demo_appends() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        app.apply(Action::OpenAddPeople(LAUNCH.into()), &ctx);
        app.apply(
            Action::AddMembers {
                chat: LAUNCH.into(),
                members: vec![person("u-alan")],
                history: true,
            },
            &ctx,
        );
        let ids = launch_members(&app);
        assert_eq!(ids.len(), 6);
        assert!(ids.contains(&"u-alan".to_owned()));
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn add_skips_existing_members() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        app.apply(
            Action::AddMembers {
                chat: LAUNCH.into(),
                members: vec![person("u-ada")],
                history: false,
            },
            &ctx,
        );
        assert_eq!(launch_members(&app).len(), 5);
    }

    #[test]
    fn leave_in_demo_removes_chat() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        app.apply(
            Action::OpenConversation(ConversationId::Chat(LAUNCH.into())),
            &ctx,
        );
        app.apply(Action::LeaveChat(LAUNCH.into()), &ctx);
        assert!(app.chats.iter().all(|c| c.id != LAUNCH));
        assert_eq!(app.open, None);
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn adding_and_leaving_work_on_a_meeting_chat_in_demo() {
        const MEETING: &str = "19:meeting_demo-design-review@thread.v2";
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        let before = app.group_members(MEETING).len();
        assert!(before > 0);
        app.apply(
            Action::AddMembers {
                chat: MEETING.into(),
                members: vec![person("u-alan")],
                history: false,
            },
            &ctx,
        );
        assert_eq!(app.group_members(MEETING).len(), before + 1);
        app.apply(
            Action::OpenConversation(ConversationId::Chat(MEETING.into())),
            &ctx,
        );
        app.apply(Action::LeaveChat(MEETING.into()), &ctx);
        assert!(app.chats.iter().all(|c| c.id != MEETING));
        assert_eq!(app.open, None);
    }

    #[test]
    fn open_leave_sets_overlay() {
        let (mut app, _dir) = demo_app();
        let ctx = egui::Context::default();
        app.apply(Action::OpenLeaveChat(LAUNCH.into()), &ctx);
        assert_eq!(app.overlay, Some(Overlay::LeaveChat(LAUNCH.into())));
    }

    #[test]
    fn leave_without_scope_shows_consent_dialog() {
        // The dialog itself decides between confirming and explaining.
        let (mut app, _commands) = group_app();
        app.granted = "User.Read Chat.ReadWrite".into();
        assert!(!app.has_scope(crate::auth::CHAT_MEMBER_SCOPE));
        let ctx = egui::Context::default();
        app.apply(Action::OpenLeaveChat("19:g".into()), &ctx);
        assert_eq!(app.overlay, Some(Overlay::LeaveChat("19:g".into())));
    }

    #[test]
    fn leave_without_scope_never_sends() {
        let (mut app, mut commands) = group_app();
        app.granted = "User.Read Chat.ReadWrite".into();
        let ctx = egui::Context::default();
        app.apply(Action::LeaveChat("19:g".into()), &ctx);
        assert_eq!(app.chats.len(), 1);
        assert!(app.leaving.is_empty());
        assert!(commands.try_recv().is_err());
        assert_eq!(app.overlay, Some(Overlay::LeaveChat("19:g".into())));
    }

    #[test]
    fn left_chat_stays_out_until_graph_stops_listing_it() {
        let (mut app, _commands) = group_app();
        let ctx = egui::Context::default();
        app.apply(Action::LeaveChat("19:g".into()), &ctx);
        app.handle(Event::LeftChat("19:g".into()), &ctx);
        app.handle(
            Event::Chats {
                chats: vec![Chat {
                    id: "19:g".into(),
                    ..Default::default()
                }],
                cached: false,
            },
            &ctx,
        );
        assert!(app.chats.is_empty(), "a lagging list must not re-add it");
        app.handle(
            Event::Chats {
                chats: vec![],
                cached: false,
            },
            &ctx,
        );
        assert!(app.left.is_empty());
    }

    #[test]
    fn people_opened_fetches_members() {
        let (mut app, mut commands) = group_app();
        let ctx = egui::Context::default();
        app.apply(Action::PeopleOpened("19:g".into()), &ctx);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::Fetch {
                resource: Resource::Members(chat),
                trigger: Trigger::Opened,
            }) if chat == "19:g"
        ));
    }

    #[test]
    fn members_event_replaces_members() {
        let (mut app, _commands) = group_app();
        let ctx = egui::Context::default();
        app.handle(
            Event::Members {
                chat: "19:g".into(),
                members: vec![person("u-me"), person("u-ada"), person("u-grace")],
            },
            &ctx,
        );
        assert_eq!(app.chats[0].members.len(), 3);
        // People still being added stay until Graph confirms them.
        app.apply(
            Action::AddMembers {
                chat: "19:g".into(),
                members: vec![person("u-alan")],
                history: false,
            },
            &ctx,
        );
        app.handle(
            Event::Members {
                chat: "19:g".into(),
                members: vec![person("u-me")],
            },
            &ctx,
        );
        let ids: Vec<&str> = app.chats[0]
            .members
            .iter()
            .map(|m| m.user_id.as_str())
            .collect();
        assert_eq!(ids, ["u-me", "u-alan"]);
    }

    #[test]
    fn add_members_marks_adding_and_sends() {
        let (mut app, mut commands) = group_app();
        let ctx = egui::Context::default();
        app.apply(
            Action::AddMembers {
                chat: "19:g".into(),
                members: vec![person("u-alan"), person("u-ada")],
                history: true,
            },
            &ctx,
        );
        assert!(
            app.adding
                .contains(&("19:g".to_owned(), "u-alan".to_owned()))
        );
        assert_eq!(app.adding.len(), 1, "existing members are skipped");
        assert_eq!(app.chats[0].members.len(), 3);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::AddMembers { chat, users, history: true })
                if chat == "19:g" && users == ["u-alan"]
        ));
    }

    #[test]
    fn members_added_clears_adding_and_toasts_failures() {
        let (mut app, _commands) = group_app();
        let ctx = egui::Context::default();
        app.apply(
            Action::AddMembers {
                chat: "19:g".into(),
                members: vec![person("u-alan"), person("u-hedy")],
                history: false,
            },
            &ctx,
        );
        app.handle(
            Event::MembersAdded {
                chat: "19:g".into(),
                added: vec!["u-alan".into()],
                failed: vec!["u-hedy".into()],
            },
            &ctx,
        );
        assert!(app.adding.is_empty());
        let ids: Vec<&str> = app.chats[0]
            .members
            .iter()
            .map(|m| m.user_id.as_str())
            .collect();
        assert_eq!(ids, ["u-me", "u-ada", "u-alan"]);
        assert!(
            app.toasts
                .iter()
                .any(|t| t.kind == ToastKind::Error && t.text == "Could not add 1 person")
        );
    }

    #[test]
    fn leave_hides_chat_and_sends() {
        let (mut app, mut commands) = group_app();
        let ctx = egui::Context::default();
        app.apply(
            Action::OpenConversation(ConversationId::Chat("19:g".into())),
            &ctx,
        );
        while commands.try_recv().is_ok() {}
        app.apply(Action::LeaveChat("19:g".into()), &ctx);
        assert!(app.chats.is_empty());
        assert!(app.leaving.contains_key("19:g"));
        assert_eq!(app.open, None);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::LeaveChat(chat)) if chat == "19:g"
        ));
        // A chat list refresh does not bring it back meanwhile.
        app.handle(
            Event::Chats {
                chats: vec![Chat {
                    id: "19:g".into(),
                    ..Default::default()
                }],
                cached: false,
            },
            &ctx,
        );
        assert!(app.chats.is_empty());
    }

    #[test]
    fn leave_failed_restores_chat() {
        let (mut app, _commands) = group_app();
        let ctx = egui::Context::default();
        app.apply(Action::LeaveChat("19:g".into()), &ctx);
        app.handle(
            Event::LeaveFailed {
                chat: "19:g".into(),
                admin: false,
            },
            &ctx,
        );
        assert_eq!(app.chats.len(), 1);
        assert!(app.leaving.is_empty());
        assert!(app.toasts.iter().any(|t| t.kind == ToastKind::Error));

        app.apply(Action::LeaveChat("19:g".into()), &ctx);
        app.handle(
            Event::LeaveFailed {
                chat: "19:g".into(),
                admin: true,
            },
            &ctx,
        );
        assert_eq!(app.chats.len(), 1);
        assert!(
            app.toasts
                .iter()
                .any(|t| t.text.contains("administrator must approve"))
        );
    }

    #[test]
    fn left_chat_forgets_copy() {
        let (mut app, _commands) = group_app();
        let ctx = egui::Context::default();
        app.apply(Action::LeaveChat("19:g".into()), &ctx);
        app.handle(Event::LeftChat("19:g".into()), &ctx);
        assert!(app.leaving.is_empty());
        assert!(app.chats.is_empty());
    }

    #[test]
    fn latest_pages_connect_to_older_ones() {
        let mut conversation = Conversation::default();
        conversation.merge(
            (0..5).map(|i| message(&i.to_string(), i)).collect(),
            Merge::Latest,
        );
        conversation.merge(
            vec![message("3", 3), message("4", 4), message("5", 5)],
            Merge::Latest,
        );
        let ids: Vec<&str> = conversation
            .messages
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["0", "1", "2", "3", "4", "5"]);
        // A page that does not connect replaces the gap-y history.
        conversation.merge(vec![message("9", 9)], Merge::Latest);
        assert_eq!(conversation.messages.len(), 1);
    }

    #[test]
    fn pasted_images_send_without_text_and_survive_retry() {
        let (mut app, mut commands) = app();
        let ctx = egui::Context::default();
        let chat = ConversationId::Chat("19:images".into());
        let image = crate::clipboard::encode_rgba(1, 1, &[0, 128, 255, 255]).unwrap();
        app.handle(
            Event::Pasted {
                conversation: chat.clone(),
                range: (0, 0),
                text: String::new(),
                image: Ok(Some(image.clone())),
            },
            &ctx,
        );
        assert_eq!(app.drafts[&chat].images.len(), 1);
        app.apply(
            Action::Send {
                conversation: chat.clone(),
                text: String::new(),
                reply_to: None,
                mentions: Vec::new(),
                files: Vec::new(),
                images: vec![image.clone()],
            },
            &ctx,
        );
        let Command::Send {
            local_id, images, ..
        } = commands.try_recv().unwrap()
        else {
            panic!("send expected")
        };
        assert_eq!(images.as_slice(), std::slice::from_ref(&image));
        assert_eq!(
            app.conversations[&chat].messages[0].body.images().count(),
            1
        );
        app.handle(
            Event::SendFailed {
                conversation: chat.clone(),
                local_id: local_id.clone(),
                reason: "offline".into(),
            },
            &ctx,
        );
        app.apply(
            Action::RetrySend {
                conversation: chat,
                local_id,
            },
            &ctx,
        );
        let Command::Send { images, .. } = commands.try_recv().unwrap() else {
            panic!("retry expected")
        };
        assert_eq!(images, [image]);
    }

    #[test]
    fn text_paste_replaces_the_selected_unicode_range_in_its_original_chat() {
        let (mut app, _) = app();
        let chat = ConversationId::Chat("19:paste".into());
        app.drafts.insert(
            chat.clone(),
            Draft {
                text: "A👋Z".into(),
                pending_pastes: 1,
                ..Default::default()
            },
        );
        app.open = Some(ConversationId::Chat("19:elsewhere".into()));
        app.handle(
            Event::Pasted {
                conversation: chat.clone(),
                range: (1, 2),
                text: "é".into(),
                image: Ok(None),
            },
            &egui::Context::default(),
        );
        assert_eq!(app.drafts[&chat].text, "AéZ");
        assert_eq!(app.drafts[&chat].paste_cursor, Some(2));
        assert_eq!(app.drafts[&chat].pending_pastes, 0);
    }

    #[test]
    fn history_uses_the_visible_boundary_and_deduplicates_inflight_requests() {
        let (mut app, mut commands) = app();
        let chat = ConversationId::Chat("19:history".into());
        app.conversations.entry(chat.clone()).or_default().messages =
            vec![message("first", 200), message("last", 300)];
        let ctx = egui::Context::default();
        app.apply(Action::LoadOlder(chat.clone()), &ctx);
        app.apply(Action::LoadOlder(chat.clone()), &ctx);
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::LoadOlder {
                before: Some(200),
                ..
            }
        ));
        assert!(commands.try_recv().is_err());
        app.handle(
            Event::Messages {
                conversation: chat.clone(),
                messages: vec![message("earlier", 100)],
                merge: Merge::Older,
                more: Some(true),
                cached: true,
            },
            &ctx,
        );
        assert!(app.conversations[&chat].restore_history_anchor);
        assert_eq!(app.conversations[&chat].messages[0].id, "earlier");
    }

    #[test]
    fn relay_keys_are_saved_by_the_service_and_reconnect_every_account() {
        let (mut app, mut account_commands) = app();
        let (service, mut service_commands) = Backend::recording();
        app.service = service;
        app.settings.relay_url = Some("https://relay.example.com".into());
        let ctx = egui::Context::default();

        // Too short: refused here, nothing stored.
        app.apply(Action::SetRelayKey(Some("short".into())), &ctx);
        assert!(service_commands.try_recv().is_err());
        assert_eq!(app.toasts.last().unwrap().kind, ToastKind::Error);

        app.form_relay_key = " office-relay-key-0123 ".into();
        app.apply(Action::SetRelayKey(Some(app.form_relay_key.clone())), &ctx);
        match service_commands.try_recv().unwrap() {
            Command::SaveRelayKey(Some(key)) => assert_eq!(key.0, "office-relay-key-0123"),
            other => panic!("unexpected {other:?}"),
        }
        // Accounts reconnect only once the key is stored.
        assert!(account_commands.try_recv().is_err());
        app.handle(
            Event::RelayKeySaved {
                set: true,
                ok: true,
            },
            &ctx,
        );
        assert!(app.settings.relay_key_set);
        assert!(app.form_relay_key.is_empty());
        assert!(matches!(
            account_commands.try_recv().unwrap(),
            Command::Relay(Some(url)) if url == "https://relay.example.com"
        ));

        // A failed store write changes nothing and says so.
        app.apply(Action::SetRelayKey(None), &ctx);
        assert!(matches!(
            service_commands.try_recv().unwrap(),
            Command::SaveRelayKey(None)
        ));
        app.handle(
            Event::RelayKeySaved {
                set: false,
                ok: false,
            },
            &ctx,
        );
        assert!(app.settings.relay_key_set);
        assert!(account_commands.try_recv().is_err());
        assert_eq!(app.toasts.last().unwrap().kind, ToastKind::Error);
    }

    #[test]
    fn updates_wait_for_an_explicit_restart_and_do_not_discard_drafts() {
        let (mut app, _) = app();
        let (service, mut commands) = Backend::recording();
        app.service = service;
        let ctx = egui::Context::default();
        app.apply(Action::CheckUpdate, &ctx);
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::CheckUpdate { download: true }
        ));
        assert!(!app.quit_requested);
        let prepared = fastframe_update::Prepared::sample(
            fastframe_update::Installation {
                executable: PathBuf::from("/tmp/chaat"),
                kind: fastframe_update::Kind::Portable,
            },
            "0.1.1",
        );
        app.handle(
            Event::Update(crate::updates::Event::Status(
                crate::updates::Status::Ready(Box::new(prepared)),
            )),
            &ctx,
        );
        app.drafts
            .entry(ConversationId::Chat("19:draft".into()))
            .or_default()
            .text = "Unsent".into();
        app.apply(Action::InstallUpdate, &ctx);
        assert!(commands.try_recv().is_err());
        assert!(!app.quit_requested);
        app.drafts.clear();
        app.apply(Action::InstallUpdate, &ctx);
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::InstallUpdate(_)
        ));
        assert!(
            !app.quit_requested,
            "only a successful helper handoff may quit"
        );
        app.handle(Event::Update(crate::updates::Event::Quit), &ctx);
        assert!(app.quit_requested);
    }

    #[test]
    fn changes_update_in_place_and_older_pages_prepend() {
        let mut conversation = Conversation::default();
        conversation.merge(vec![message("5", 5), message("6", 6)], Merge::Latest);
        let mut edited = message("6", 6);
        edited.edited = true;
        conversation.merge(vec![edited, message("7", 7)], Merge::Changes);
        conversation.merge(vec![message("1", 1)], Merge::Older);
        let ids: Vec<&str> = conversation
            .messages
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["1", "5", "6", "7"]);
        assert!(conversation.messages[2].edited);
    }

    #[test]
    fn sending_is_optimistic_and_reconciled() {
        let (mut app, mut commands) = app();
        let chat = ConversationId::Chat("19:a".into());
        let ctx = egui::Context::default();
        app.apply(
            Action::Send {
                conversation: chat.clone(),
                text: "**hi**".into(),
                reply_to: None,
                mentions: Vec::new(),
                files: Vec::new(),
                images: Vec::new(),
            },
            &ctx,
        );
        let shown = &app.conversations[&chat].messages[0];
        assert_eq!(shown.delivery, Delivery::Sending);
        let Ok(Command::Send { local_id, html, .. }) = commands.try_recv() else {
            panic!("send command expected");
        };
        assert_eq!(html, "<p><strong>hi</strong></p>");
        let mut confirmed = message("1700", crate::time::now());
        confirmed.author_id = Some("u-me".into());
        app.handle(
            Event::Sent {
                conversation: chat.clone(),
                local_id,
                message: Box::new(confirmed),
            },
            &ctx,
        );
        let messages = &app.conversations[&chat].messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "1700");
        assert_eq!(messages[0].delivery, Delivery::Sent);
    }

    #[test]
    fn reactions_toggle_for_the_signed_in_person() {
        let mut reactions = Vec::new();
        toggle_reaction(&mut reactions, "👍", "me", true);
        toggle_reaction(&mut reactions, "👍", "ada", true);
        assert_eq!(reactions[0].users, ["me", "ada"]);
        toggle_reaction(&mut reactions, "👍", "me", false);
        toggle_reaction(&mut reactions, "👍", "ada", false);
        assert!(reactions.is_empty());
    }

    #[test]
    fn opening_a_chat_fetches_it_and_marks_it_read() {
        let (mut app, mut commands) = app();
        app.chats = vec![Chat {
            id: "19:a".into(),
            preview: Some(crate::model::Preview {
                message_id: "1".into(),
                author_id: Some("u-ada".into()),
                at: 10,
                ..Default::default()
            }),
            last_read: Some(1),
            ..Default::default()
        }];
        let ctx = egui::Context::default();
        app.apply(
            Action::OpenConversation(ConversationId::Chat("19:a".into())),
            &ctx,
        );
        let mut seen = Vec::new();
        while let Ok(command) = commands.try_recv() {
            seen.push(format!("{command:?}"));
        }
        assert!(
            seen.iter()
                .any(|c| c
                    .starts_with("Fetch { resource: Messages(Chat(\"19:a\")), trigger: Opened"))
        );
        assert!(seen.iter().any(|c| c == "MarkRead(\"19:a\")"));
        assert!(!app.chats[0].is_unread("u-me"));
    }

    #[test]
    fn losing_the_connection_is_a_quiet_marker_not_a_toast() {
        let (mut app, _commands) = app();
        let ctx = egui::Context::default();
        app.handle(Event::Reachable(false), &ctx);
        assert!(app.unreachable);
        assert!(app.toasts.is_empty());
        app.handle(Event::Reachable(true), &ctx);
        assert!(!app.unreachable);
    }

    #[test]
    fn messages_arriving_in_the_background_stay_unread() {
        let (mut app, mut commands) = app();
        let chat = ConversationId::Chat("19:a".into());
        app.chats = vec![Chat {
            id: "19:a".into(),
            preview: Some(crate::model::Preview {
                message_id: "1".into(),
                author_id: Some("u-ada".into()),
                at: 10,
                ..Default::default()
            }),
            last_read: Some(20),
            ..Default::default()
        }];
        app.open = Some(chat.clone());
        app.window_focused = false;
        let ctx = egui::Context::default();
        let incoming = |id: &str, at| Message {
            author_id: Some("u-ada".into()),
            ..message(id, at)
        };
        app.handle(
            Event::Messages {
                conversation: chat.clone(),
                messages: vec![incoming("2", 300)],
                merge: Merge::Latest,
                more: None,
                cached: false,
            },
            &ctx,
        );
        let mut seen = Vec::new();
        while let Ok(command) = commands.try_recv() {
            seen.push(format!("{command:?}"));
        }
        assert!(!seen.iter().any(|c| c.starts_with("MarkRead")));
        assert!(app.chats[0].is_unread("u-me"));

        app.window_focused = true;
        app.handle(
            Event::Messages {
                conversation: chat,
                messages: vec![incoming("3", 400)],
                merge: Merge::Latest,
                more: None,
                cached: false,
            },
            &ctx,
        );
        let mut seen = Vec::new();
        while let Ok(command) = commands.try_recv() {
            seen.push(format!("{command:?}"));
        }
        assert!(seen.iter().any(|c| c == "MarkRead(\"19:a\")"));
        assert!(!app.chats[0].is_unread("u-me"));
    }

    #[test]
    fn only_teams_links_are_joined() {
        let (mut app, _commands) = app();
        let ctx = egui::Context::default();
        app.apply(Action::JoinMeeting("https://evil.example/x".into()), &ctx);
        assert_eq!(app.toasts.len(), 1);
    }

    #[test]
    fn mentions_and_files_travel_with_a_send() {
        let (mut app, mut commands) = app();
        let chat = ConversationId::Chat("19:a".into());
        app.chats = vec![Chat {
            id: "19:a".into(),
            members: vec![
                Member {
                    tenant_id: None,
                    user_id: "u-me".into(),
                    display_name: "Me".into(),
                    email: Some("me@contoso.com".into()),
                },
                Member {
                    tenant_id: None,
                    user_id: "u-ada".into(),
                    display_name: "Ada".into(),
                    email: Some("ada@contoso.com".into()),
                },
            ],
            ..Default::default()
        }];
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plan.pdf");
        std::fs::write(&file, b"%PDF").unwrap();
        let ctx = egui::Context::default();
        app.apply(
            Action::Send {
                conversation: chat.clone(),
                text: "@Ada see the plan".into(),
                reply_to: None,
                mentions: vec![MentionRef {
                    user_id: "u-ada".into(),
                    name: "Ada".into(),
                }],
                files: vec![file.clone()],
                images: Vec::new(),
            },
            &ctx,
        );
        let Ok(Command::Send {
            html,
            mentions,
            files,
            recipients,
            ..
        }) = commands.try_recv()
        else {
            panic!("send command expected");
        };
        assert_eq!(html, "<p><at id=\"0\">Ada</at> see the plan</p>");
        assert_eq!(mentions.len(), 1);
        assert_eq!(files, vec![file]);
        assert_eq!(recipients, vec!["ada@contoso.com".to_owned()]);
        let shown = &app.conversations[&chat].messages[0];
        assert_eq!(shown.attachments[0].name.as_deref(), Some("plan.pdf"));
    }

    #[test]
    fn out_of_office_asks_for_its_scope_then_fetches_and_saves() {
        use crate::model::{AutoReplies, OutOfOffice};
        let (mut app, mut commands) = app();
        app.granted = "User.Read Chat.ReadWrite ChatMember.ReadWrite".into();
        let ctx = egui::Context::default();
        app.apply(Action::OpenOutOfOffice, &ctx);
        assert_eq!(app.settings_page, Some(SettingsPage::OutOfOffice));
        assert_eq!(
            app.overlay,
            Some(Overlay::Consent(crate::auth::MAILBOX_SETTINGS_SCOPE))
        );
        assert!(commands.try_recv().is_err(), "no fetch without the scope");

        // Consent arrives while the page is open: look once.
        app.handle(
            Event::Granted("User.Read MailboxSettings.ReadWrite".into()),
            &ctx,
        );
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::Fetch {
                resource: Resource::OutOfOffice,
                trigger: Trigger::Opened
            })
        ));
        // A later session restore with the scope already held looks no more.
        app.handle(
            Event::Granted("User.Read MailboxSettings.ReadWrite".into()),
            &ctx,
        );
        assert!(commands.try_recv().is_err());
        let away = OutOfOffice {
            replies: AutoReplies::On,
            internal: "Away".into(),
            ..Default::default()
        };
        app.handle(Event::OutOfOffice(away.clone()), &ctx);
        assert_eq!(app.form_out_of_office.internal, "Away");

        // Saving shows as under way until Graph answers; a failure keeps
        // the edit so the person can try again.
        app.form_out_of_office.replies = AutoReplies::Off;
        let off = app.form_out_of_office.value().unwrap();
        app.apply(Action::SetOutOfOffice(off.clone()), &ctx);
        assert!(app.out_of_office_saving);
        assert!(matches!(commands.try_recv(), Ok(Command::SetOutOfOffice(v)) if v == off));
        app.apply(Action::SetOutOfOffice(off.clone()), &ctx);
        assert!(commands.try_recv().is_err(), "one save at a time");
        app.handle(Event::OutOfOfficeNotSaved, &ctx);
        assert!(!app.out_of_office_saving);
        assert_eq!(app.form_out_of_office.replies, AutoReplies::Off);
        assert!(app.form_out_of_office.changed_from(&away));

        // A fetch while the person edits leaves the edit alone.
        app.handle(Event::OutOfOffice(away.clone()), &ctx);
        assert_eq!(app.form_out_of_office.replies, AutoReplies::Off);
        app.handle(Event::OutOfOffice(off.clone()), &ctx);
        assert_eq!(app.out_of_office, Some(off));
        assert_eq!(app.form_out_of_office.replies, AutoReplies::Off);

        app.handle(Event::OutOfOfficeUnavailable, &ctx);
        assert!(app.no_mailbox);
    }

    #[test]
    fn features_needing_new_permissions_ask_first() {
        let (mut app, mut commands) = app();
        app.granted = "User.Read Chat.ReadWrite ChatMember.ReadWrite".into();
        let ctx = egui::Context::default();
        app.apply(Action::OpenNewChat, &ctx);
        assert_eq!(
            app.overlay,
            Some(Overlay::Consent(crate::auth::CHAT_CREATE_SCOPE))
        );
        app.apply(Action::SetAvailability(Some(Availability::Busy)), &ctx);
        assert_eq!(
            app.overlay,
            Some(Overlay::Consent(crate::auth::PRESENCE_WRITE_SCOPE))
        );
        app.apply(
            Action::RequestScope(crate::auth::PRESENCE_WRITE_SCOPE),
            &ctx,
        );
        let Ok(Command::SignIn { extra_scopes, .. }) = commands.try_recv() else {
            panic!("consent sign-in expected");
        };
        assert_eq!(extra_scopes, vec!["Presence.ReadWrite".to_owned()]);
        app.granted.push_str(" Presence.ReadWrite");
        app.apply(Action::SetAvailability(Some(Availability::Busy)), &ctx);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::SetAvailability(Some(Availability::Busy)))
        ));
        assert_eq!(
            app.my_presence.as_ref().unwrap().availability,
            Availability::Busy
        );
    }

    #[test]
    fn hidden_accounts_update_without_touching_the_screen() {
        let (mut app, _commands) = app();
        let (second, _second_commands) = Backend::recording();
        app.accounts.push(Account::new(second));
        app.accounts[1].auth = AuthStatus::SignedIn {
            account: "u-me2.t2".into(),
            name: "Me Too".into(),
            username: "me@fabrikam.com".into(),
        };
        app.accounts[1].open = Some(ConversationId::Chat("19:b".into()));
        let ctx = egui::Context::default();
        // An event for account 1 while account 0 shows.
        app.active = 1;
        app.hidden = true;
        app.handle(
            Event::Chats {
                chats: vec![Chat {
                    id: "19:b".into(),
                    ..Default::default()
                }],
                cached: false,
            },
            &ctx,
        );
        app.hidden = false;
        app.active = 0;
        assert!(app.chats.is_empty());
        assert_eq!(app.accounts[1].chats.len(), 1);
        app.apply(Action::SwitchAccount(1), &ctx);
        assert_eq!(app.active, 1);
        assert_eq!(app.chats[0].id, "19:b");
        assert_eq!(app.label(), "Me Too");
    }

    #[test]
    fn signing_in_twice_keeps_one_slot() {
        let (mut app, _commands) = app();
        app.accounts[0].id = Some("u-me.t1".into());
        let (second, _second_commands) = Backend::recording();
        app.accounts.push(Account::new(second));
        app.active = 1;
        let ctx = egui::Context::default();
        app.handle(
            Event::Auth(AuthStatus::SignedIn {
                account: "u-me.t1".into(),
                name: "Me".into(),
                username: "me@contoso.com".into(),
            }),
            &ctx,
        );
        app.retire_duplicates();
        app.apply_actions(&ctx);
        assert_eq!(app.accounts.len(), 1);
        assert_eq!(app.active, 0);
    }

    #[test]
    fn unread_only_lists_unread_chats_and_keeps_one_read_while_open() {
        let (mut app, _commands) = app();
        let chat = |id: &str, last_read: i64| Chat {
            id: id.into(),
            preview: Some(crate::model::Preview {
                message_id: "1".into(),
                author_id: Some("u-ada".into()),
                at: 10,
                ..Default::default()
            }),
            last_read: Some(last_read),
            ..Default::default()
        };
        app.chats = vec![chat("19:unread", 1), chat("19:read", 10)];
        let me = app.me().to_owned();
        let shown = |app: &App| -> Vec<String> {
            app.chats
                .iter()
                .filter(|c| crate::ui::chat_list::listed(app, c, &me))
                .map(|c| c.id.clone())
                .collect()
        };
        assert_eq!(shown(&app).len(), 2);
        app.unread_only = true;
        assert_eq!(shown(&app), vec!["19:unread".to_owned()]);
        let ctx = egui::Context::default();
        // An open chat that was already read does not show.
        app.apply(
            Action::OpenConversation(ConversationId::Chat("19:read".into())),
            &ctx,
        );
        assert_eq!(shown(&app), vec!["19:unread".to_owned()]);
        // Opening an unread chat reads it, and it stays while open.
        app.apply(
            Action::OpenConversation(ConversationId::Chat("19:unread".into())),
            &ctx,
        );
        assert!(!app.chats[0].is_unread(&me));
        assert_eq!(shown(&app), vec!["19:unread".to_owned()]);
        // Once another chat opens, it goes.
        app.apply(
            Action::OpenConversation(ConversationId::Chat("19:read".into())),
            &ctx,
        );
        assert!(shown(&app).is_empty());
    }

    #[test]
    fn an_expired_account_signs_in_again_over_its_cached_chats() {
        let (mut app, mut commands) = app();
        app.chats = vec![Chat {
            id: "19:a".into(),
            ..Default::default()
        }];
        let expired = AuthStatus::Expired {
            username: "me@contoso.com".into(),
            reason: "Sign in again to get new messages.".into(),
        };
        app.on_auth(expired.clone());
        assert!(app.shows_workspace());
        let ctx = egui::Context::default();

        // Cancelling goes back to the cached workspace.
        app.apply(Action::SignIn, &ctx);
        assert!(matches!(commands.try_recv(), Ok(Command::SignIn { .. })));
        app.on_auth(AuthStatus::WaitingForBrowser { url: "u".into() });
        assert!(app.shows_workspace());
        app.apply(Action::CancelSignIn, &ctx);
        assert_eq!(app.auth, expired);
        assert!(app.reauth.is_none());

        // A failure keeps the workspace and says why.
        app.apply(Action::SignIn, &ctx);
        app.on_auth(AuthStatus::Failed("declined".into()));
        assert!(matches!(
            &app.auth,
            AuthStatus::Expired { username, reason }
                if username == "me@contoso.com" && reason == "declined"
        ));
        assert!(app.shows_workspace());

        app.apply(Action::SignIn, &ctx);
        app.on_auth(AuthStatus::SignedIn {
            account: "u-me.t1".into(),
            name: "Me".into(),
            username: "me@contoso.com".into(),
        });
        assert!(app.reauth.is_none());
        assert!(app.is_signed_in());
    }

    fn option_spec(
        id: &str,
        kind: crate::plugin::protocol::OptionKind,
        default: Option<serde_json::Value>,
    ) -> crate::plugin::protocol::OptionSpec {
        crate::plugin::protocol::OptionSpec {
            id: id.into(),
            kind,
            label: id.into(),
            default,
            choices: Vec::new(),
            ..Default::default()
        }
    }

    /// An enabled entry the person has not decided anything about.
    fn undecided_entry(app: &mut App, id: &str) {
        plugin_entry(app, id, &[]);
        let entry = app.settings.plugins.last_mut().unwrap();
        entry.approved_needs.clear();
    }

    fn accounts_sent_to(commands: &[Command], id: &str) -> bool {
        commands.iter().any(|c| {
            matches!(
                c,
                Command::PluginSend { id: to, message: crate::plugin::protocol::HostMessage::Accounts { .. } }
                    if to == id
            )
        })
    }

    fn presence_sent_to(commands: &[Command], id: &str) -> bool {
        commands.iter().any(|c| {
            matches!(
                c,
                Command::PluginSend { id: to, message: crate::plugin::protocol::HostMessage::Presence { .. } }
                    if to == id
            )
        })
    }

    #[test]
    fn events_for_unknown_accounts_are_dropped() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        call_alerts_plugin(&mut app, "ca");
        sent(&mut commands);
        let stranger = Some("u-stranger.t9");
        app.handle(
            plugin_event(&app, "ca", sign_in_event(stranger, "AB12")),
            &ctx,
        );
        app.handle(plugin_event(&app, "ca", signed_in_event(stranger)), &ctx);
        app.handle(
            plugin_event(
                &app,
                "ca",
                crate::plugin::event::PluginEvent::Listening {
                    account: "u-stranger.t9".into(),
                },
            ),
            &ctx,
        );
        app.handle(
            plugin_event(
                &app,
                "ca",
                crate::plugin::event::PluginEvent::Error {
                    account: stranger.map(str::to_owned),
                    message: "nope".into(),
                },
            ),
            &ctx,
        );
        let mut alert = ring("stranger-call", "u-ada");
        alert.account = stranger.map(str::to_owned);
        app.handle(
            plugin_event(
                &app,
                "ca",
                crate::plugin::event::PluginEvent::IncomingCall(alert),
            ),
            &ctx,
        );
        assert!(app.plugins["ca"].sign_in.is_empty());
        assert!(app.plugins["ca"].listening.is_empty());
        assert!(app.toasts.is_empty());
        assert!(app.rings.is_empty());
        // The same events for one of Chaat's accounts count.
        app.handle(
            plugin_event(&app, "ca", sign_in_event(Some("u-me.t1"), "AB12")),
            &ctx,
        );
        app.handle(
            plugin_event(
                &app,
                "ca",
                crate::plugin::event::PluginEvent::Listening {
                    account: "u-me.t1".into(),
                },
            ),
            &ctx,
        );
        assert_eq!(app.plugins["ca"].sign_in.len(), 1);
        assert_eq!(app.plugins["ca"].listening, ["u-me.t1"]);
    }

    #[test]
    fn a_new_need_waits_for_approval() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        app.my_presence = Some(busy());
        undecided_entry(&mut app, "light");
        app.sync_plugins();
        sent(&mut commands);
        let ready = |app: &mut App| {
            app.handle(
                plugin_event(
                    app,
                    "light",
                    ready_event(&["accounts", "presence"], &[], Vec::new(), &[]),
                ),
                &ctx,
            );
        };
        ready(&mut app);
        let after = sent(&mut commands);
        assert!(!accounts_sent_to(&after, "light"));
        assert!(!presence_sent_to(&after, "light"));
        assert_eq!(app.plugins["light"].pending_needs, ["accounts", "presence"]);
        assert!(app.plugin_receives("light").is_empty());
        assert_eq!(app.toasts.len(), 1);
        assert_eq!(
            app.toasts[0].text,
            "Plugin \u{201c}light\u{201d} asks for access. Open Settings \u{203a} Plugins to allow it."
        );
        assert!(
            !app.wants_show,
            "asking for access does not raise the window"
        );
        // The same pending set does not announce itself again.
        app.toasts.clear();
        ready(&mut app);
        assert!(app.toasts.is_empty());
        assert!(!accounts_sent_to(&sent(&mut commands), "light"));
    }

    #[test]
    fn approved_needs_flow_and_denied_ones_dont() {
        let ctx = egui::Context::default();
        let ready = |app: &mut App| {
            app.handle(
                plugin_event(
                    app,
                    "light",
                    ready_event(&["accounts", "presence"], &[], Vec::new(), &[]),
                ),
                &ctx,
            );
        };
        let (mut app, mut commands) = app_with_service();
        app.my_presence = Some(busy());
        undecided_entry(&mut app, "light");
        app.sync_plugins();
        ready(&mut app);
        sent(&mut commands);
        app.apply(Action::ApprovePluginNeeds("light".into()), &ctx);
        let after = sent(&mut commands);
        assert!(accounts_sent_to(&after, "light"));
        assert!(presence_sent_to(&after, "light"));
        let entry = &app.settings.plugins[0];
        assert_eq!(entry.approved_needs, ["accounts", "presence"]);
        assert!(entry.denied_needs.is_empty());
        assert!(app.plugins["light"].pending_needs.is_empty());
        assert_eq!(app.plugin_receives("light"), ["accounts", "presence"]);
        // A restart hello carries the accounts of an approved plugin.
        app.settings.plugins[0].path = PathBuf::from("other.exe");
        app.sync_plugins();
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::StartPlugin { hello: crate::plugin::protocol::HostMessage::Hello { accounts, .. }, .. }
                if accounts.len() == 1
        )));

        // A refusal sends nothing, and the same `ready` does not ask again.
        let (mut app, mut commands) = app_with_service();
        app.my_presence = Some(busy());
        undecided_entry(&mut app, "light");
        app.sync_plugins();
        ready(&mut app);
        sent(&mut commands);
        app.apply(Action::DenyPluginNeeds("light".into()), &ctx);
        assert_eq!(
            app.settings.plugins[0].denied_needs,
            ["accounts", "presence"]
        );
        assert!(app.settings.plugins[0].approved_needs.is_empty());
        app.toasts.clear();
        ready(&mut app);
        let after = sent(&mut commands);
        assert!(!accounts_sent_to(&after, "light"));
        assert!(!presence_sent_to(&after, "light"));
        assert!(app.plugins["light"].pending_needs.is_empty());
        assert!(app.toasts.is_empty());
        assert!(app.plugin_receives("light").is_empty());
    }

    #[test]
    fn a_changed_file_waits_and_trust_resets_needs() {
        use crate::plugin::event::PluginEvent;
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "light", &["accounts"]);
        {
            let entry = &mut app.settings.plugins[0];
            entry.sha256 = Some("old".into());
            entry.denied_needs = vec!["presence".into()];
        }
        app.sync_plugins();
        assert!(matches!(
            sent(&mut commands).first(),
            Some(Command::StartPlugin { expected_sha256: Some(pin), .. }) if pin == "old"
        ));
        app.handle(
            plugin_event(
                &app,
                "light",
                PluginEvent::FileChanged {
                    sha256: "new".into(),
                },
            ),
            &ctx,
        );
        assert_eq!(app.plugins["light"].file_changed.as_deref(), Some("new"));
        assert!(!app.plugins["light"].running);
        assert!(
            sent(&mut commands).is_empty(),
            "the changed file is not run"
        );
        app.sync_plugins();
        assert!(sent(&mut commands).is_empty(), "nor restarted by a sync");

        app.apply(Action::TrustPluginFile("light".into()), &ctx);
        let entry = &app.settings.plugins[0];
        assert_eq!(entry.sha256.as_deref(), Some("new"));
        assert!(entry.approved_needs.is_empty() && entry.denied_needs.is_empty());
        assert!(app.plugins["light"].file_changed.is_none());
        assert!(matches!(
            sent(&mut commands).first(),
            Some(Command::StartPlugin { expected_sha256: Some(pin), hello: crate::plugin::protocol::HostMessage::Hello { accounts, .. }, .. })
                if pin == "new" && accounts.is_empty()
        ));

        // Keeping it off turns the plugin off and drops the question.
        app.handle(
            plugin_event(
                &app,
                "light",
                PluginEvent::FileChanged {
                    sha256: "newer".into(),
                },
            ),
            &ctx,
        );
        app.apply(Action::KeepPluginOff("light".into()), &ctx);
        assert!(!app.settings.plugins[0].enabled);
        assert!(app.plugins["light"].file_changed.is_none());
        assert_eq!(app.settings.plugins[0].sha256.as_deref(), Some("new"));
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::StopPlugin { id } if id == "light"
        )));
    }

    #[test]
    fn the_first_fingerprint_is_stored_and_never_replaced() {
        use crate::plugin::event::PluginEvent;
        let (mut app, _commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "light", &[]);
        let fingerprint = |app: &mut App, sha: &str| {
            app.handle(
                plugin_event(
                    app,
                    "light",
                    PluginEvent::Fingerprint { sha256: sha.into() },
                ),
                &ctx,
            );
        };
        fingerprint(&mut app, "first");
        assert_eq!(app.settings.plugins[0].sha256.as_deref(), Some("first"));
        fingerprint(&mut app, "second");
        assert_eq!(app.settings.plugins[0].sha256.as_deref(), Some("first"));
    }

    #[test]
    fn a_late_secret_answer_for_an_old_instance_is_ignored() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "light", &[]);
        app.settings.plugins[0].instance = "new".into();
        app.plugins.entry("light".into()).or_default().options.push(
            crate::plugin::protocol::OptionSpec {
                id: "key".into(),
                kind: crate::plugin::protocol::OptionKind::Secret,
                label: "Key".into(),
                ..Default::default()
            },
        );
        let answer = |instance: &str| Event::PluginSecretSaved {
            plugin: "light".into(),
            instance: instance.into(),
            option: "key".into(),
            set: true,
            ok: true,
        };
        app.handle(answer("old"), &ctx);
        assert!(app.settings.plugins[0].secrets.is_empty());
        assert!(sent(&mut commands).is_empty());
        app.handle(answer("new"), &ctx);
        assert_eq!(app.settings.plugins[0].secrets, ["key"]);
        // Saves carry the entry's instance.
        app.apply(
            Action::SetPluginSecret {
                plugin: "light".into(),
                option: "key".into(),
                value: Some("v".into()),
            },
            &ctx,
        );
        assert!(sent(&mut commands).iter().any(|c| matches!(
            c,
            Command::SavePluginSecret { instance, .. } if instance == "new"
        )));
        // Each new entry gets its own instance.
        app.add_plugin(PathBuf::from("C:/x/other.exe"));
        app.add_plugin(PathBuf::from("D:/y/other.exe"));
        let instances: Vec<&str> = app
            .settings
            .plugins
            .iter()
            .skip(1)
            .map(|e| e.instance.as_str())
            .collect();
        assert!(instances.iter().all(|i| i.len() == 32));
        assert_ne!(instances[0], instances[1]);
    }

    #[test]
    fn removing_deletes_every_described_secret() {
        use crate::plugin::protocol::OptionKind;
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "light", &[]);
        app.settings.plugins[0].secrets = vec!["recorded".into()];
        app.sync_plugins();
        sent(&mut commands);
        app.handle(
            plugin_event(
                &app,
                "light",
                ready_event(
                    &[],
                    &[],
                    vec![
                        option_spec("recorded", OptionKind::Secret, None),
                        option_spec("described", OptionKind::Secret, None),
                        option_spec("plain", OptionKind::Text, None),
                    ],
                    &[],
                ),
            ),
            &ctx,
        );
        sent(&mut commands);
        app.apply(Action::RemovePlugin("light".into()), &ctx);
        let deleted: Vec<String> = sent(&mut commands)
            .into_iter()
            .filter_map(|c| match c {
                Command::SavePluginSecret {
                    plugin,
                    instance,
                    option,
                    value: None,
                } if plugin == "light" && instance == "inst-light" => Some(option),
                _ => None,
            })
            .collect();
        assert_eq!(deleted.len(), 2);
        assert!(deleted.contains(&"recorded".to_owned()));
        assert!(deleted.contains(&"described".to_owned()));
    }

    #[test]
    fn forget_needs_the_accounts_need() {
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "allowed", &["accounts"]);
        plugin_entry(&mut app, "unallowed", &["accounts"]);
        app.settings.plugins[1].approved_needs.clear();
        for entry in &mut app.settings.plugins {
            entry.forget = vec!["u-queued.t2".into()];
        }
        app.sync_plugins();
        let started = sent(&mut commands);
        let hello_forget = |id: &str| {
            started.iter().find_map(|c| match c {
                Command::StartPlugin {
                    id: started,
                    hello: crate::plugin::protocol::HostMessage::Hello { forget, .. },
                    ..
                } if started == id => Some(forget.clone()),
                _ => None,
            })
        };
        assert_eq!(
            hello_forget("allowed"),
            Some(vec!["u-queued.t2".to_owned()])
        );
        assert_eq!(hello_forget("unallowed"), Some(Vec::new()));
        app.apply(Action::SignOut, &ctx);
        let after = sent(&mut commands);
        let forgets = |id: &str| {
            after
                .iter()
                .filter(|c| {
                    matches!(
                        c,
                        Command::PluginSend { id: to, message: crate::plugin::protocol::HostMessage::Forget { .. } }
                            if to == id
                    )
                })
                .count()
        };
        assert_eq!(forgets("allowed"), 1);
        assert_eq!(forgets("unallowed"), 0, "an unallowed plugin is not told");
        assert!(
            !app.settings.plugins[1]
                .forget
                .contains(&"u-me.t1".to_owned()),
            "nor is the account queued for it"
        );
        assert!(
            app.settings.plugins[0]
                .forget
                .contains(&"u-me.t1".to_owned())
        );
    }

    #[test]
    fn options_follow_the_current_schema() {
        use crate::plugin::protocol::OptionKind;
        let (mut app, mut commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "light", &[]);
        {
            let entry = &mut app.settings.plugins[0];
            entry
                .options
                .insert("ip".into(), serde_json::json!("10.0.0.5"));
            entry.options.insert("old".into(), serde_json::json!(1));
            entry
                .options
                .insert("was_plain".into(), serde_json::json!("leaked"));
            entry.secrets = vec!["key".into(), "gone".into(), "ip".into()];
        }
        app.sync_plugins();
        sent(&mut commands);
        app.handle(
            plugin_event(
                &app,
                "light",
                ready_event(
                    &[],
                    &[],
                    vec![
                        option_spec("ip", OptionKind::Text, None),
                        option_spec("was_plain", OptionKind::Secret, None),
                        option_spec("key", OptionKind::Secret, None),
                    ],
                    &[],
                ),
            ),
            &ctx,
        );
        let (values, secrets) = sent(&mut commands)
            .into_iter()
            .find_map(|c| match c {
                Command::PluginOptions {
                    id,
                    values,
                    secrets,
                } if id == "light" => Some((values, secrets)),
                _ => None,
            })
            .expect("options after ready");
        assert_eq!(
            values.keys().collect::<Vec<_>>(),
            ["ip"],
            "only described plain options"
        );
        assert_eq!(secrets, ["key"], "only secret options with a stored value");
        let stored = &app.settings.plugins[0].options;
        assert!(
            !stored.contains_key("was_plain"),
            "a plain value for an option that is now secret is deleted"
        );
        assert!(stored.contains_key("old"), "unknown options are withheld");
        assert!(stored.contains_key("ip"));
    }

    #[test]
    fn plugin_toasts_are_quoted_and_limited() {
        let (mut app, _commands) = app_with_service();
        let ctx = egui::Context::default();
        plugin_entry(&mut app, "light", &[]);
        app.plugins.entry("light".into()).or_default().info.name =
            Some("Busy \u{201d}light".into());
        let stopped = |app: &mut App, id: &str, message: &str| {
            app.handle(
                plugin_event(
                    app,
                    id,
                    crate::plugin::event::PluginEvent::Status {
                        running: false,
                        message: Some(message.into()),
                    },
                ),
                &ctx,
            );
        };
        stopped(&mut app, "light", "first");
        assert_eq!(app.toasts.len(), 1);
        assert_eq!(
            app.toasts[0].text,
            "Plugin \u{201c}Busy 'light\u{201d}: first"
        );
        stopped(&mut app, "light", "second");
        assert_eq!(app.toasts.len(), 1, "a second toast within 10 s is dropped");
        // Another plugin is not held back by this one.
        plugin_entry(&mut app, "other", &[]);
        stopped(&mut app, "other", "hi");
        assert_eq!(app.toasts.len(), 2);
        // After the gap, the plugin may toast again.
        if let Some(past) =
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(11))
        {
            app.plugin_toasts.insert("light".into(), past);
            stopped(&mut app, "light", "third");
            assert!(app.toasts.iter().any(|t| t.text.ends_with(": third")));
        }
    }
}
