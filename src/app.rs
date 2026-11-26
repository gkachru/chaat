//! Application state and the one place it changes.
//!
//! Views in `ui/` read [`App`] and push [`Action`]s; [`App::apply`] runs
//! them after the frame. Backend [`crate::backend::Event`]s are applied at the start of each
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

use crate::backend::{
    AccountCommand, AccountEvent, AuthStatus, Backend, Merge, ServiceCommand, ServiceEvent,
    ServiceRole, Waker,
};
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

mod actions;
mod agents;
mod calls;
mod chat_actions;
pub mod chat_filter;
mod events;
mod history;
mod notify;
use notify::NoticeItem;
mod pins;
mod plugins;
mod push;
mod setup;
mod setup_events;
mod setup_export;
mod status;
mod unified;
pub use calls::Docked;
pub use history::HistoryLayout;
pub use pins::message_link;
pub use plugins::{ADD_START_LIMIT, PluginState, PluginSummary, PluginsView, SummaryLevel};
pub use push::{PushPluginState, PushPluginView};
pub use status::expiry_millis;

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
    /// When each thread's replies were last asked for, by root id.
    pub replies_asked: HashMap<String, Instant>,
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

impl Draft {
    /// What is saved of the composer: text, quote and mentions. `None` for
    /// an empty composer; files, images and an edit in progress are not kept.
    fn persisted(&self) -> Option<crate::outbox::StoredDraft> {
        (!self.text.trim().is_empty() && self.editing.is_none()).then(|| {
            crate::outbox::StoredDraft {
                text: self.text.clone(),
                reply_to: self.reply_to.clone(),
                mentions: self.mentions.clone(),
            }
        })
    }

    /// Whether the saved copy already holds what the composer shows.
    fn is_saved(&self, saved: Option<&crate::outbox::StoredDraft>) -> bool {
        match (self.persisted(), saved) {
            (None, None) => true,
            (Some(own), Some(saved)) => own == *saved,
            _ => false,
        }
    }
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
    /// Hits per account index while Unified Chats is shown.
    pub unified_hits: Vec<(usize, SearchHit)>,
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
    /// The chat list shows only the chats the person hid.
    pub hidden_only: bool,
    /// Pinned messages by chat id, as Graph last listed them.
    pub pins: HashMap<String, Vec<crate::model::PinnedMessage>>,
    /// Chats the person is hiding (`true`) or unhiding (`false`), awaiting
    /// Graph, so a stale chat list does not undo it.
    pub pending_hide: HashMap<String, bool>,
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
    /// Presence updates; shown while `push` is live.
    pub push_presence: crate::realtime::PresenceFollow,

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
    /// What the backend holds of each conversation's draft.
    saved_drafts: HashMap<ConversationId, crate::outbox::StoredDraft>,
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
            hidden_only: false,
            pins: HashMap::new(),
            pending_hide: HashMap::new(),
            groups_only: false,
            meetings_only: false,
            unread_kept: None,
            drafts: HashMap::new(),
            push: crate::realtime::PushStatus::Off,
            push_presence: crate::realtime::PresenceFollow::Connecting,
            browser_profile: format!("Chaat {}", crate::auth::random_token(4)),
            pending_sends: HashMap::new(),
            saved_drafts: HashMap::new(),
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
            AuthStatus::SignedIn { account, .. } => crate::auth::session::user_id(account),
            _ => self
                .id
                .as_deref()
                .map(crate::auth::session::user_id)
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
    pub service: Backend<ServiceRole>,
    pub update: crate::updates::Status,
    pub update_receipt: Option<fastframe_update::Receipt>,
    pub dirs: AppDirs,
    pub settings: Settings,
    /// Unified Chats is showing (the logo tile) instead of one account.
    pub unified: bool,
    /// Filters of the Unified chat list.
    pub unified_filter: crate::model::UnifiedFilter,
    settings_dirty: bool,
    pub waker: Waker,
    pub demo: bool,
    /// Where notifications are shown (recorded in the demo).
    pub desktop: crate::notify::Desktop,
    urgent: crate::notify::schedule::UrgentRepeats,
    seen: crate::notify::schedule::Seen,
    /// The newest message time already toasted, per account and conversation.
    notified_newest: HashMap<(String, String), i64>,
    /// Reminders snoozed: meeting id and when to show again.
    snoozed: Vec<(String, i64)>,
    /// Meeting chats the person took part in this run.
    participated: HashSet<String>,
    /// For requesting taskbar attention from notification code.
    notify_ctx: Option<egui::Context>,

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
    /// Find in the open conversation, while its bar is open.
    pub find: Option<crate::model::FindState>,
    /// The Microsoft Search half of find, waiting for typing to pause: the
    /// conversation, the query and when it is due. Never more than one.
    find_remote: Option<(ConversationId, String, Instant)>,
    pub toasts: Vec<Toast>,
    /// Ringing calls the call-alerts plugin reported, newest last.
    pub rings: Vec<Ring>,
    /// Which rings already raised the window, and when it last was.
    ring_raises: calls::RingRaises,
    /// Pending proposals from the agents plugin, by account. Process-wide.
    pub agent_proposals: crate::agents::proposals::Proposals,
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
    /// The account each toast with an action belongs to, by account id, so
    /// Undo lands on it after the person switched accounts.
    toast_accounts: HashMap<u64, Option<String>>,
    pub actions: Vec<Action>,

    /// Forms.
    pub form_client_id: String,
    pub form_tenant: String,
    pub form_relay: String,
    /// The relay access key being typed. Cleared once saved; never written
    /// to the settings file.
    pub form_relay_key: String,
    pub form_status: String,
    /// How long the status message being typed lasts.
    pub status_expiry: crate::model::StatusExpiry,
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
            self.service.send(ServiceCommand::DiscardUpdate(prepared));
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
    pub fn attach_service(&mut self, service: Backend<ServiceRole>) {
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
            status_expiry: Default::default(),
            form_out_of_office: Default::default(),
            dirs,
            unified: settings.unified,
            unified_filter: Default::default(),
            settings,
            settings_dirty: false,
            desktop: if demo {
                crate::notify::Desktop::recording()
            } else {
                crate::notify::Desktop::new(waker.clone())
            },
            urgent: Default::default(),
            seen: Default::default(),
            notified_newest: Default::default(),
            snoozed: Vec::new(),
            participated: HashSet::new(),
            notify_ctx: None,
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
            find: None,
            find_remote: None,
            toasts: Vec::new(),
            rings: Vec::new(),
            ring_raises: calls::RingRaises::default(),
            plugins: Default::default(),
            plugins_started: Default::default(),
            plugin_generations: Default::default(),
            push_plugin: Default::default(),
            plugin_generation: 0,
            plugin_toasts: Default::default(),
            agent_proposals: Default::default(),
            plugin_option_errors: Default::default(),
            plugins_view: PluginsView::default(),
            reordering: false,
            plugin_started_at: Default::default(),
            next_toast: 0,
            toast_accounts: HashMap::new(),
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
                .send(AccountCommand::Relay(self.settings.relay_url.clone()));
        }
    }

    fn start_account(&mut self, index: usize) {
        let configured = self.settings.has_app_registration();
        let start = AccountCommand::Start {
            client_id: self.settings.client_id.clone(),
            tenant: self.settings.tenant.clone(),
            account: self.accounts[index].id.clone(),
        };
        let relay = AccountCommand::Relay(self.settings.relay_url.clone());
        let session = AccountCommand::PresenceSession(self.settings.presence_session);
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
        let mut account = None;
        if self.hidden && self.accounts.len() > 1 {
            text = format!("{}: {text}", self.account_look(self.active).name);
            account = Some(self.active);
        }
        if self.toasts.iter().any(|t| t.text == text) {
            return;
        }
        self.next_toast += 1;
        self.toasts.push(Toast {
            id: self.next_toast,
            kind,
            text,
            action: None,
            account,
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    /// The index of an account by its id.
    pub(crate) fn account_index(&self, id: &str) -> Option<usize> {
        self.accounts
            .iter()
            .position(|a| a.id.as_deref() == Some(id))
    }

    /// Makes the account the active one while `f` runs, the way events of a
    /// background account are applied.
    pub(crate) fn in_account<R>(&mut self, index: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let (active, hidden) = (self.active, self.hidden);
        self.active = index;
        self.hidden = index != active || hidden;
        let result = f(self);
        self.active = active;
        self.hidden = hidden;
        result
    }

    /// Sends a command to the active account's backend; the demo has none.
    pub(crate) fn send_account(&self, command: AccountCommand) {
        if !self.demo {
            self.backend.send(command);
        }
    }

    fn fetch(&self, resource: Resource, trigger: Trigger) {
        self.send_account(AccountCommand::Fetch { resource, trigger });
    }

    /// Asks for whatever the current view shows.
    fn fetch_view(&self, trigger: Trigger) {
        self.fetch_section(trigger);
        if let Some(open) = &self.open {
            self.fetch(Resource::Messages(open.clone()), trigger);
        }
    }

    /// Asks for the list of the current section only.
    fn fetch_section(&self, trigger: Trigger) {
        match self.section {
            Section::Chats => self.fetch(Resource::Chats, trigger),
            Section::Teams => self.fetch(Resource::Teams, trigger),
            Section::Calendar => self.fetch(Resource::Calendar, trigger),
            Section::Calls => {}
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
            self.handle_service(event, ctx);
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
            self.in_account(index, |app| {
                for event in events {
                    app.handle(event, ctx);
                }
            });
        }
        self.active = shown.min(self.accounts.len().saturating_sub(1));
        self.retire_duplicates();
        self.handle_tray();
        self.handle_requests();
        self.handle_clicks();
        self.remind(ctx);
        self.tick_urgent(ctx);
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
                    account.backend.send(AccountCommand::PresenceActive);
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

    // -- Events ------------------------------------------------------------------

    /// Applies one event to the account at `self.active`.
    pub fn handle(&mut self, event: AccountEvent, ctx: &egui::Context) {
        if self.notify_ctx.is_none() {
            self.notify_ctx = Some(ctx.clone());
        }
        match event {
            AccountEvent::AgentAnswer {
                plugin,
                request,
                data,
            } => self.on_agent_answer(plugin, request, data),
            AccountEvent::Pasted {
                conversation,
                range,
                text,
                image,
            } => self.handle_pasted(conversation, range, text, image, ctx),
            AccountEvent::Auth(status) => self.on_auth(status),
            AccountEvent::Profile(profile) => self.profile = Some(*profile),
            AccountEvent::Granted(granted) => self.handle_granted(granted),
            AccountEvent::ChannelAccess(access) => self.channel_access = access,
            AccountEvent::Chats { chats, cached } => self.on_chats(chats, cached),
            AccountEvent::Teams { teams, .. } => self.handle_teams(teams),
            AccountEvent::Channels { team, channels } => self.handle_channels(team, channels),
            AccountEvent::Messages {
                conversation,
                messages,
                merge,
                more,
                cached,
            } => self.on_messages(conversation, messages, merge, more, cached),
            AccountEvent::Replies {
                conversation,
                root,
                replies,
            } => self.handle_replies(conversation, root, replies),
            AccountEvent::Sent {
                conversation,
                local_id,
                message,
            } => self.on_sent(conversation, local_id, *message),
            AccountEvent::SendFailed {
                conversation,
                local_id,
                reason,
            } => self.handle_send_failed(conversation, local_id, reason),
            AccountEvent::Deleted {
                conversation,
                message,
            } => self.handle_deleted(conversation, message),
            AccountEvent::Presence(list) => self.handle_presence(list),
            AccountEvent::MyPresence(presence) => self.handle_my_presence(presence),
            AccountEvent::OutOfOffice(value) => self.handle_out_of_office(value),
            AccountEvent::OutOfOfficeUnavailable => {
                self.no_mailbox = true;
                self.out_of_office_saving = false;
            }
            AccountEvent::OutOfOfficeNotSaved => self.out_of_office_saving = false,
            AccountEvent::MeetingChat { join_url, chat } => self.on_meeting_chat(join_url, chat),
            AccountEvent::MeetingById { id, join_url } => self.on_meeting_by_id(id, join_url, ctx),
            AccountEvent::ChatHidden { chat, hidden, ok } => self.on_chat_hidden(chat, hidden, ok),
            AccountEvent::MarkUnreadFailed(chat) => self.on_mark_unread_failed(chat),
            AccountEvent::Pins { chat, pins } => self.on_pins(chat, pins),
            AccountEvent::UserName { user, name } => {
                if let Some(name) = name {
                    self.names.insert(user, name);
                }
            }
            AccountEvent::Photo { user, bytes } => self.handle_photo(user, bytes, ctx),
            AccountEvent::Media { url, bytes } => self.handle_media(url, bytes, ctx),
            AccountEvent::Meetings { meetings, .. } => self.meetings = meetings,
            // handled in T3
            AccountEvent::Recordings(_) => {}
            AccountEvent::SearchResults {
                query,
                hits,
                remote,
            } => self.handle_search_results(query, hits, remote),
            AccountEvent::People { query, people } => self.handle_people(query, people),
            AccountEvent::Members { chat, members } => self.handle_members(chat, members),
            AccountEvent::MembersAdded {
                chat,
                added,
                failed,
            } => self.handle_members_added(chat, added, failed),
            AccountEvent::LeftChat(chat) => self.handle_left_chat(chat),
            AccountEvent::LeaveFailed { chat, admin } => self.handle_leave_failed(chat, admin),
            AccountEvent::ChatCreated(chat) => self.handle_chat_created(chat),
            AccountEvent::Loading(resource, on) => self.handle_loading(resource, on),
            AccountEvent::Reachable(reachable) => self.unreachable = !reachable,
            AccountEvent::Error(text) => self.toast(ToastKind::Error, text),
            AccountEvent::Push(status) => {
                if status != crate::realtime::PushStatus::Live {
                    self.push_presence = crate::realtime::PresenceFollow::Connecting;
                }
                self.push = status;
            }
            AccountEvent::PushPresence(follow) => self.push_presence = follow,
            AccountEvent::Restored { drafts, outbox } => self.on_restored(drafts, outbox),
            AccountEvent::ConversationHits {
                conversation,
                query,
                hits,
            } => self.on_conversation_hits(conversation, query, hits),
        }
    }

    /// Applies an event of the service backend. These never swap accounts:
    /// they belong to the app, not to one signed-in person.
    pub fn handle_service(&mut self, event: ServiceEvent, ctx: &egui::Context) {
        if self.notify_ctx.is_none() {
            self.notify_ctx = Some(ctx.clone());
        }
        match event {
            ServiceEvent::Update(event) => self.handle_update(event),
            event @ (ServiceEvent::SetupOpened { .. }
            | ServiceEvent::SetupApplied { .. }
            | ServiceEvent::SetupExported(_)) => self.on_setup_event(event),
            ServiceEvent::RelayKeySaved { set, ok } => self.handle_relay_key_saved(set, ok),
            ServiceEvent::Plugin {
                id,
                generation,
                event,
            } => self.on_plugin_event(id, generation, event),
            ServiceEvent::PluginSecretSaved {
                plugin,
                instance,
                option,
                set,
                ok,
            } => self.on_plugin_secret_saved(&plugin, &instance, option, set, ok),
        }
    }

    /// How long the find query stays unchanged before Microsoft Search runs.
    const FIND_REMOTE_DELAY: Duration = Duration::from_millis(500);
    /// A thread expanded again this soon shows the replies already loaded.
    const REPLIES_REUSE: Duration = Duration::from_secs(60);

    // -- Actions -------------------------------------------------------------------

    pub fn apply_actions(&mut self, ctx: &egui::Context) {
        let mut rounds = 0;
        while !self.actions.is_empty() && rounds < 8 {
            rounds += 1;
            for action in std::mem::take(&mut self.actions) {
                self.apply(action, ctx);
            }
        }
        self.sync_drafts();
        self.send_due_find_search(Instant::now(), ctx);
    }

    pub fn apply(&mut self, action: Action, ctx: &egui::Context) {
        self.apply_one(action, ctx);
        self.prune_agent_proposals();
        self.plugins_view_follows_settings();
    }

    fn apply_one(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::CheckUpdate => self.check_update(self.settings.automatic_updates),
            Action::SetAutomaticUpdates(enabled) => {
                self.settings.automatic_updates = enabled;
                self.mark_settings_dirty();
            }
            Action::DownloadUpdate => self.apply_download_update(),
            Action::InstallUpdate => self.apply_install_update(),
            Action::Navigate(section) => {
                self.apply_navigate(section);
                if self.unified_view() {
                    self.fan_out_view(Trigger::Opened);
                }
            }
            Action::ShowUnified => self.apply_show_unified(),
            Action::ShowAccount(index) => self.apply_show_account(index),
            Action::SelectMeeting(id) => self.selected_meeting = Some(id),
            Action::OpenSharedChat(index, id) => self.apply_open_shared_chat(index, id, ctx),
            Action::SetUnifiedOnly(only) => self.apply_set_unified_only(only),
            Action::ForAccount(index, action) => self.apply_for_account(index, *action, ctx),
            Action::SetAccountLook(index, look) => self.apply_set_account_look(index, look),
            Action::OpenConversation(conversation) => self.open_conversation(conversation),
            Action::OpenMessage {
                conversation,
                message,
            } => self.apply_open_message(conversation, message),
            Action::ToggleTeam(team) => self.apply_toggle_team(team),
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
            } => self.apply_edit(conversation, message, text),
            Action::Delete {
                conversation,
                message,
            } => self.apply_delete(conversation, message),
            Action::React {
                conversation,
                message,
                emoji,
                on,
            } => self.apply_react(conversation, message, emoji, on),
            Action::StartReply(message) => self.apply_start_reply(message),
            Action::StartEdit(message) => self.apply_start_edit(message),
            Action::CancelCompose => self.apply_cancel_compose(),
            Action::LoadOlder(conversation) => self.apply_load_older(conversation),
            Action::LoadReplies {
                conversation,
                message,
            } => self.apply_load_replies(conversation, message),
            Action::ToggleThread(root) => self.apply_toggle_thread(root),
            Action::RetrySend {
                conversation,
                local_id,
            } => self.apply_retry_send(conversation, local_id),
            Action::Find(Some(query)) => self.find_query(query),
            Action::Find(None) => {
                self.find = None;
                self.find_remote = None;
            }
            Action::FindNext(step) => self.apply_find_next(step, ctx),
            Action::JumpToLatest => self.scroll_to_end = 4,
            Action::DeleteFailed {
                conversation,
                local_id,
            } => self.apply_delete_failed(conversation, local_id),
            Action::AttachFiles => self.apply_attach_files(),
            Action::Paste {
                conversation,
                range,
                text,
            } => self.apply_paste(conversation, range, text),
            Action::AddFiles(paths) => self.add_files(paths),
            Action::RemoveFile(index) => self.apply_remove_file(index),
            Action::LoadMedia(url) => self.apply_load_media(url, ctx),
            Action::LoadPhotos(users) => self.apply_load_photos(users, ctx),
            Action::SignIn => self.apply_sign_in(),
            Action::SignInWithCode => self.backend.send(AccountCommand::SignIn {
                extra_scopes: Vec::new(),
                device_code: true,
            }),
            Action::CancelSignIn => self.apply_cancel_sign_in(),
            Action::SignOut => self.sign_out(),
            Action::RequestScope(scope) => self.apply_request_scope(scope),
            Action::SaveAppRegistration { client_id, tenant } => {
                self.apply_save_app_registration(client_id, tenant)
            }
            Action::SetAvailability(availability) => self.apply_set_availability_all(availability),
            Action::OpenStatusMessage => self.apply_open_status_message(),
            Action::SetStatusMessage(text) => self.apply_set_status_message(text),
            Action::SetStatusExpiry(expiry) => self.status_expiry = expiry,
            Action::HideChat(chat) => self.hide_chat(chat),
            Action::UnhideChat(chat) => self.unhide_chat(chat),
            Action::MarkChatUnread(chat) => self.mark_chat_unread(chat),
            Action::MarkChatRead(chat) => self.mark_chat_read(chat),
            Action::ToastAction(toast) => self.run_toast_action(toast),
            Action::PinMessage { chat, message } => self.pin_message(chat, message),
            Action::UnpinMessage { chat, pin } => self.unpin_message(chat, pin),
            Action::CopyMessageLink {
                conversation,
                message,
            } => self.copy_message_link(conversation, message, ctx),
            Action::OpenOutOfOffice => self.apply_open_out_of_office(ctx),
            Action::SetOutOfOffice(value) => self.apply_set_out_of_office(value),
            Action::SetPresenceSession(on) => self.apply_set_presence_session(on),
            Action::SwitchAccount(index) => self.apply_switch_account(index),
            Action::AddAccount => self.apply_add_account(),
            Action::CancelAddAccount => self.apply_cancel_add_account(),
            Action::OpenNewChat => self.apply_open_new_chat(),
            Action::OpenAddPeople(chat) => self.apply_open_add_people(chat),
            Action::AddMembers {
                chat,
                members,
                history,
            } => self.apply_add_members(chat, members, history),
            Action::OpenLeaveChat(chat) => self.overlay = Some(Overlay::LeaveChat(chat)),
            Action::LeaveChat(chat) => self.apply_leave_chat(chat),
            Action::PeopleOpened(chat) => {
                self.fetch(Resource::Members(chat), Trigger::Opened);
            }
            Action::SearchPeople(query) => self.apply_search_people(query),
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
            Action::Setup(action) => self.apply_setup_action(action),
            Action::JoinOnPhone => self.join_on_phone(),
            Action::PrepareTeamsWeb => self.prepare_teams_web(),
            Action::SetCallSettings(calls) => self.set_call_settings(calls),
            Action::OpenUrl(url) => self.apply_open_url(url),
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
            Action::PreviewImage(uri) => self.apply_preview_image(uri, ctx),
            Action::SearchMessages(query) => self.apply_search_messages(query),
            Action::OpenSettings => {
                let page = self.last_settings_page;
                self.apply(Action::ShowSettingsPage(page), ctx);
            }
            Action::OpenShortcuts => {
                self.apply(Action::ShowSettingsPage(SettingsPage::Shortcuts), ctx)
            }
            Action::ShowSettingsPage(page) => self.apply_show_settings_page(page),
            Action::CloseSettings => self.settings_page = None,
            Action::SetTheme(choice) => {
                self.settings.theme = choice;
                self.mark_settings_dirty();
            }
            Action::SetNotify(settings) => self.set_notify(settings),
            Action::MuteChat(chat, muted) => self.mute_chat(chat, muted),
            Action::FollowAvailable { user, name, on } => self.follow_available(user, name, on),
            Action::PauseNotifications(choice) => self.pause_notifications(choice),
            Action::TestNotification => self.send_test_notification(),
            Action::SetNotifications(on) => {
                self.settings.notifications = on;
                self.mark_settings_dirty();
            }
            Action::SetKeepRunning(on) => self.apply_set_keep_running(on),
            Action::SetCustomTitlebar(on) => self.apply_set_custom_titlebar(on, ctx),
            Action::SetRelay(url) => self.apply_set_relay(url),
            Action::SetRelayKey(key) => self.apply_set_relay_key(key),
            Action::LookUpNames(users) => self.apply_look_up_names(users),
            Action::RetryRelay => self.apply_retry_relay(),
            Action::TogglePerfOverlay => self.apply_toggle_perf_overlay(),
            Action::OpenLogFolder => self.apply_open_log_folder(),
            Action::Refresh => {
                self.apply_refresh();
                if self.unified_view() {
                    self.fan_out_view(Trigger::Refresh);
                }
            }
            Action::DismissToast(id) => self.toasts.retain(|t| t.id != id),
            Action::DismissRing(id) => self.rings.retain(|r| r.id != id),
            Action::Plugin(action) => self.apply_plugin_action(action),
            Action::Agent(action) => self.apply_agent_action(action, ctx),
            Action::RingPlaceCall(id) => self.ring_place_call(&id, ctx),
            Action::RingOpenChat(id) => self.ring_open_chat(&id, ctx),
            Action::Quit => {
                self.quit_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
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
pub(crate) mod tests;
