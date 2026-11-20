//! Chaat's own types. Views and the store never see Graph's JSON shapes; the
//! Graph layer translates into these in `graph::convert`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::html::RichText;

/// Unix time in milliseconds. Graph timestamps are converted on arrival so
/// sorting and grouping never parse strings in the frame.
pub type Millis = i64;

/// A Microsoft account signed in to Chaat: `<object id>.<tenant id>`, as MSAL
/// names a home account. `demo` for the offline sample data.
pub type AccountId = String;

/// Where a message lives.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConversationId {
    /// A one-on-one, group or meeting chat (`19:...@thread.v2`).
    Chat(String),
    /// A channel of a team. Channel posts are threads: replies carry their
    /// root's id in [`Message::reply_to`].
    Channel { team: String, channel: String },
}

impl ConversationId {
    /// The key the store files messages under.
    pub fn key(&self) -> String {
        match self {
            Self::Chat(id) => format!("chat:{id}"),
            Self::Channel { team, channel } => format!("channel:{team}:{channel}"),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        if let Some(id) = key.strip_prefix("chat:") {
            return Some(Self::Chat(id.to_owned()));
        }
        let rest = key.strip_prefix("channel:")?;
        // Team ids are GUIDs (no colon); channel ids contain colons.
        let (team, channel) = rest.split_once(':')?;
        Some(Self::Channel {
            team: team.to_owned(),
            channel: channel.to_owned(),
        })
    }

    pub fn chat_id(&self) -> Option<&str> {
        match self {
            Self::Chat(id) => Some(id),
            Self::Channel { .. } => None,
        }
    }

    pub fn is_channel(&self) -> bool {
        matches!(self, Self::Channel { .. })
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// The signed-in person.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub display_name: String,
    pub mail: Option<String>,
    pub user_principal_name: String,
    pub job_title: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChatKind {
    #[default]
    OneOnOne,
    Group,
    Meeting,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub user_id: String,
    pub display_name: String,
    pub email: Option<String>,
    /// The person's home organisation, when Graph says (chat members). People
    /// from other organisations can't be followed by a presence subscription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
}

/// The newest message of a chat, as the chat list shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub message_id: String,
    pub author_id: Option<String>,
    pub author: Option<String>,
    pub text: String,
    pub at: Millis,
    pub deleted: bool,
    /// A system event, shown in italics without an author.
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub membership: Option<Membership>,
}

/// A call or meeting starting or ending in a chat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallEvent {
    pub meeting: bool,
    pub ended: bool,
    /// How long it lasted ("8m 18s"), for an ended call.
    #[serde(default)]
    pub duration: Option<String>,
}

/// Someone named in a system event, with the name when Graph gave one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPerson {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// How a chat's members changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberChange {
    #[default]
    Added,
    Removed,
    Joined,
    Left,
}

/// "people" at the start of a sentence.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// People added to or removed from a chat. Graph often leaves the names
/// out, so the ids stay for the interface to name them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub change: MemberChange,
    #[serde(default)]
    pub by: Option<EventPerson>,
    #[serde(default)]
    pub members: Vec<EventPerson>,
}

impl Membership {
    /// "Ada added Grace to the chat." `name` fills in names Graph left out.
    pub fn describe(&self, name: impl Fn(&str) -> Option<String>) -> String {
        let named = |person: &EventPerson| {
            person
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .or_else(|| name(&person.id))
        };
        let by = self
            .by
            .as_ref()
            .and_then(named)
            .unwrap_or_else(|| "Someone".into());
        let members: Vec<String> = self
            .members
            .iter()
            .map(|person| named(person).unwrap_or_else(|| "someone".into()))
            .collect();
        let members = match members.as_slice() {
            [] => "people".to_owned(),
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        };
        match self.change {
            MemberChange::Added => format!("{by} added {members} to the chat."),
            MemberChange::Removed => format!("{by} removed {members} from the chat."),
            MemberChange::Joined => format!("{} joined.", capitalized(&members)),
            MemberChange::Left => format!("{} left.", capitalized(&members)),
        }
    }

    /// Ids of the people Graph gave no name for.
    pub fn unnamed(&self) -> impl Iterator<Item = &str> {
        self.by
            .iter()
            .chain(&self.members)
            .filter(|p| p.name.as_deref().is_none_or(str::is_empty))
            .map(|p| p.id.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub kind: ChatKind,
    /// The chat's name, when it has one (group and meeting chats).
    pub topic: Option<String>,
    pub members: Vec<Member>,
    pub preview: Option<Preview>,
    /// When the signed-in person last read the chat (Graph `viewpoint`).
    pub last_read: Option<Millis>,
    pub updated: Millis,
    pub web_url: Option<String>,
    /// A meeting chat's join link.
    pub join_url: Option<String>,
    /// Hidden from the list by the person (Graph `viewpoint.isHidden`).
    pub hidden: bool,
}

/// The person's chat with themselves, Teams' notes. Graph serves its
/// messages but not the chat itself (that answers 400), and the id is not
/// in Microsoft's documentation; Chaat builds the list entry itself.
pub const NOTES_CHAT: &str = "48:notes";

impl Chat {
    pub fn is_notes(&self) -> bool {
        self.id == NOTES_CHAT
    }

    /// Whose avatar the chat shows: the other person of a one-on-one chat,
    /// or the person themselves in their notes.
    pub fn face<'a>(&'a self, me: &str) -> Option<&'a Member> {
        if self.is_notes() {
            self.members.first()
        } else {
            self.peer(me)
        }
    }

    /// Order in the list: newest activity first, notes included.
    pub fn list_order(&self) -> std::cmp::Reverse<Millis> {
        std::cmp::Reverse(self.activity())
    }

    /// What the list and the header call the chat: its topic, or the other
    /// members' names.
    pub fn title(&self, me: &str) -> String {
        if let Some(topic) = self.topic.as_deref().filter(|t| !t.trim().is_empty()) {
            return topic.to_owned();
        }
        let mut others: Vec<&str> = self
            .members
            .iter()
            .filter(|member| member.user_id != me)
            .map(|member| member.display_name.as_str())
            .filter(|name| !name.is_empty())
            .collect();
        // Graph lists members in no promised order, and different requests
        // for the same chat disagree; sorting keeps the title from changing.
        others.sort_by_cached_key(|name| name.to_lowercase());
        match others.len() {
            // Someone who left the organization: Graph drops them from the
            // members, or keeps them without a name.
            0 if self.kind == ChatKind::OneOnOne => "Unknown user".to_owned(),
            0 => "Just you".to_owned(),
            1..=3 => others.join(", "),
            n => format!("{}, {} and {} others", others[0], others[1], n - 2),
        }
    }

    /// The other person of a one-on-one chat.
    pub fn peer<'a>(&'a self, me: &str) -> Option<&'a Member> {
        if self.kind != ChatKind::OneOnOne {
            return None;
        }
        self.members.iter().find(|member| member.user_id != me)
    }

    /// Whether the newest message arrived after the person last read it and
    /// was written by someone else.
    pub fn is_unread(&self, me: &str) -> bool {
        let Some(preview) = &self.preview else {
            return false;
        };
        if preview.author_id.as_deref() == Some(me) {
            return false;
        }
        self.last_read.is_none_or(|read| preview.at > read)
    }

    /// Sorting key for the list: the newest message, the time the list
    /// shows. Graph's `lastUpdatedDateTime` also moves for membership and
    /// other chat changes, so it orders only chats without a message.
    pub fn activity(&self) -> Millis {
        self.preview
            .as_ref()
            .map_or(self.updated, |preview| preview.at)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub web_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelKind {
    #[default]
    Standard,
    Private,
    Shared,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: ChannelKind,
    pub web_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    /// The emoji itself. Graph's legacy names (`like`, `heart`, ...) are
    /// mapped to their emoji on arrival.
    pub emoji: String,
    pub users: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentKind {
    /// A file shared through OneDrive or SharePoint.
    File,
    /// The message this one replies to (Graph `messageReference`).
    Quote {
        message_id: String,
        author: Option<String>,
        preview: String,
    },
    /// An Adaptive Card or other card Chaat does not draw.
    Card,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub kind: AttachmentKind,
    pub name: Option<String>,
    pub content_type: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delivery {
    /// Confirmed by Graph.
    #[default]
    Sent,
    /// Shown optimistically, waiting for Graph.
    Sending,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation: Option<ConversationId>,
    /// A channel reply's root post.
    pub reply_to: Option<String>,
    pub author_id: Option<String>,
    pub author: String,
    pub created: Millis,
    pub modified: Millis,
    pub edited: bool,
    pub deleted: bool,
    /// A system event ("Ada added Grace to the chat"), drawn as a line.
    pub system: bool,
    /// A call someone else placed that nobody answered (see
    /// `graph::convert::call_missed`), offered a "Call back".
    #[serde(default)]
    pub missed_call: bool,
    /// For people added or removed: who, by id, to name them.
    #[serde(default)]
    pub membership: Option<Membership>,
    /// For a call or meeting starting or ending.
    #[serde(default)]
    pub call: Option<CallEvent>,
    pub subject: Option<String>,
    pub importance_high: bool,
    pub body: RichText,
    pub reactions: Vec<Reaction>,
    pub attachments: Vec<Attachment>,
    /// Channel posts: replies loaded with the root.
    pub replies: Vec<Message>,
    pub delivery: Delivery,
}

impl Message {
    pub fn is_from(&self, user: &str) -> bool {
        self.author_id.as_deref() == Some(user)
    }

    /// What the chat list shows for a message: its text, or its files.
    pub fn preview_text(&self) -> String {
        let text = self.body.summary(140);
        if !text.is_empty() {
            return text;
        }
        let files: Vec<&str> = self
            .attachments
            .iter()
            .filter(|a| matches!(a.kind, AttachmentKind::File))
            .filter_map(|a| a.name.as_deref())
            .collect();
        if files.is_empty() {
            String::new()
        } else {
            format!("📎 {}", files.join(", "))
        }
    }

    pub fn quote(&self) -> Option<&Attachment> {
        self.attachments
            .iter()
            .find(|attachment| matches!(attachment.kind, AttachmentKind::Quote { .. }))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Availability {
    Available,
    Busy,
    DoNotDisturb,
    BeRightBack,
    Away,
    Offline,
    #[default]
    Unknown,
}

impl Availability {
    pub fn from_graph(value: &str) -> Self {
        match value {
            "Available" | "AvailableIdle" => Self::Available,
            "Busy" | "BusyIdle" => Self::Busy,
            "DoNotDisturb" => Self::DoNotDisturb,
            "BeRightBack" => Self::BeRightBack,
            "Away" => Self::Away,
            "Offline" => Self::Offline,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::Busy => "Busy",
            Self::DoNotDisturb => "Do not disturb",
            Self::BeRightBack => "Be right back",
            Self::Away => "Away",
            Self::Offline => "Offline",
            Self::Unknown => "Status unknown",
        }
    }

    /// The value `setUserPreferredPresence` takes, for the ones a person can
    /// choose.
    pub fn graph_preferred(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Available => Some(("Available", "Available")),
            Self::Busy => Some(("Busy", "Busy")),
            Self::DoNotDisturb => Some(("DoNotDisturb", "DoNotDisturb")),
            Self::BeRightBack => Some(("BeRightBack", "BeRightBack")),
            Self::Away => Some(("Away", "Away")),
            Self::Offline => Some(("Offline", "OffWork")),
            Self::Unknown => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub availability: Availability,
    /// Graph's `activity` (`InACall`, `InAMeeting`, `Presenting`, ...).
    pub activity: Option<String>,
    pub status_message: Option<String>,
}

impl Presence {
    pub fn describe(&self) -> String {
        match self.activity.as_deref() {
            Some("InACall") => "In a call".into(),
            Some("InAConferenceCall") => "In a conference call".into(),
            Some("InAMeeting") => "In a meeting".into(),
            Some("Presenting") => "Presenting".into(),
            Some("OutOfOffice") => "Out of office".into(),
            Some("Inactive") => "Inactive".into(),
            Some("Focusing") => "Focusing".into(),
            _ => self.availability.label().into(),
        }
    }
}

/// Whether the mailbox sends automatic replies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AutoReplies {
    #[default]
    Off,
    On,
    /// Only between [`OutOfOffice::start`] and [`OutOfOffice::end`].
    Scheduled,
}

/// Who outside the organization gets the external reply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ExternalAudience {
    #[default]
    None,
    Contacts,
    All,
}

impl ExternalAudience {
    pub const ALL: [Self; 3] = [Self::None, Self::Contacts, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Don't reply outside my organization",
            Self::Contacts => "Reply to my contacts only",
            Self::All => "Reply to everyone outside my organization",
        }
    }
}

/// The mailbox's automatic replies (Out of Office). Teams shows the person
/// as out of office while they are on; Outlook sends the same replies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutOfOffice {
    pub replies: AutoReplies,
    /// The scheduled window, when Exchange has one.
    pub start: Option<Millis>,
    pub end: Option<Millis>,
    /// Replies as plain text.
    pub internal: String,
    pub external: String,
    pub audience: ExternalAudience,
}

impl OutOfOffice {
    /// Whether replies go out at `now`.
    pub fn active(&self, now: Millis) -> bool {
        match self.replies {
            AutoReplies::Off => false,
            AutoReplies::On => true,
            AutoReplies::Scheduled => {
                self.start.is_some_and(|start| start <= now)
                    && self.end.is_some_and(|end| now < end)
            }
        }
    }

    /// What the form edits: the window only when scheduled (Exchange keeps
    /// an unused one), to the minute.
    fn normalized(&self) -> Self {
        let minute = |at: Option<Millis>| at.map(|at| at - at.rem_euclid(60_000));
        let scheduled = self.replies == AutoReplies::Scheduled;
        Self {
            start: if scheduled { minute(self.start) } else { None },
            end: if scheduled { minute(self.end) } else { None },
            internal: self.internal.trim().to_owned(),
            external: self.external.trim().to_owned(),
            ..self.clone()
        }
    }
}

/// The Out of Office settings page's fields: dates and times as typed, in
/// the computer's time zone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutOfOfficeForm {
    pub replies: AutoReplies,
    pub audience: ExternalAudience,
    pub internal: String,
    pub external: String,
    /// `2026-11-24`.
    pub start_date: String,
    /// `09:00`.
    pub start_time: String,
    pub end_date: String,
    pub end_time: String,
}

impl OutOfOfficeForm {
    /// The fields for `value`. Without a window, a scheduled one would start
    /// at the next half hour and last a day.
    pub fn new(value: &OutOfOffice, now: Millis) -> Self {
        const HALF_HOUR: Millis = 30 * 60_000;
        let next = now - now.rem_euclid(HALF_HOUR) + HALF_HOUR;
        let start = value.start.unwrap_or(next);
        let end = value.end.unwrap_or(start + 24 * 60 * 60_000);
        let (start_date, start_time) = crate::time::local_date_time(start);
        let (end_date, end_time) = crate::time::local_date_time(end);
        Self {
            replies: value.replies,
            audience: value.audience,
            internal: value.internal.clone(),
            external: value.external.clone(),
            start_date,
            start_time,
            end_date,
            end_time,
        }
    }

    /// The settings to save, or why they can't be.
    pub fn value(&self) -> Result<OutOfOffice, &'static str> {
        let (start, end) = if self.replies == AutoReplies::Scheduled {
            let start = crate::time::from_local(&self.start_date, &self.start_time);
            let end = crate::time::from_local(&self.end_date, &self.end_time);
            let (Some(start), Some(end)) = (start, end) else {
                return Err("Enter dates like 2026-11-24 and times like 09:00.");
            };
            if end <= start {
                return Err("The end must be after the start.");
            }
            (Some(start), Some(end))
        } else {
            (None, None)
        };
        Ok(OutOfOffice {
            replies: self.replies,
            start,
            end,
            internal: self.internal.trim().to_owned(),
            external: self.external.trim().to_owned(),
            audience: self.audience,
        })
    }

    /// Whether the fields differ from `saved`.
    pub fn changed_from(&self, saved: &OutOfOffice) -> bool {
        !self
            .value()
            .is_ok_and(|value| value.normalized() == saved.normalized())
    }
}

/// An upcoming calendar event.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub subject: String,
    pub start: Millis,
    pub end: Millis,
    pub all_day: bool,
    pub location: Option<String>,
    pub organizer: Option<String>,
    /// The Teams join link, for online meetings.
    pub join_url: Option<String>,
    pub web_link: Option<String>,
    pub cancelled: bool,
    pub preview: Option<String>,
}

/// Where a feature stands for the signed-in account, from the scopes the
/// token carries and the answers Graph gave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    #[default]
    Unknown,
    Granted,
    /// The scope was not granted; usually tenant admin consent is needed.
    NeedsConsent,
    /// Graph refused (403) even though the scope looked granted.
    Forbidden,
}

/// A person mentioned in a message being written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MentionRef {
    pub user_id: String,
    pub name: String,
}

/// A message found by search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub conversation: ConversationId,
    pub message_id: String,
    pub author: String,
    pub snippet: String,
    pub at: Millis,
}

/// A page of the settings view, which takes the place of the sidebar and
/// the open conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    #[default]
    Account,
    OutOfOffice,
    General,
    Appearance,
    Calls,
    Push,
    Shortcuts,
    Advanced,
    About,
}

impl SettingsPage {
    pub const ALL: [Self; 9] = [
        Self::Account,
        Self::OutOfOffice,
        Self::General,
        Self::Appearance,
        Self::Calls,
        Self::Push,
        Self::Shortcuts,
        Self::Advanced,
        Self::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Account => "Account",
            Self::OutOfOffice => "Out of office",
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Calls => "Calls and meetings",
            Self::Push => "Push updates",
            Self::Shortcuts => "Keyboard shortcuts",
            Self::Advanced => "Advanced",
            Self::About => "About",
        }
    }
}

/// The rail's destinations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Section {
    #[default]
    Chats,
    Teams,
    Calendar,
    Calls,
}

impl Section {
    pub const ALL: [Self; 4] = [Self::Chats, Self::Teams, Self::Calendar, Self::Calls];

    pub fn label(self) -> &'static str {
        match self {
            // The chat list carries the app's name, like its rail logo.
            Self::Chats => "Chaat",
            Self::Teams => "Teams",
            Self::Calendar => "Calendar",
            Self::Calls => "Calls",
        }
    }
}

/// What a view asks the app to do. Views push these; `App::apply` runs them
/// after the frame, so no view mutates state it is drawing from.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    CheckUpdate,
    DownloadUpdate,
    InstallUpdate,
    SetAutomaticUpdates(bool),
    Navigate(Section),
    OpenConversation(ConversationId),
    /// Opens a conversation scrolled to a message (search, notifications).
    OpenMessage {
        conversation: ConversationId,
        message: String,
    },
    ToggleTeam(String),
    CloseConversation,
    /// Moves the selection in the current list (Alt+Up/Down).
    Step(i32),

    Send {
        conversation: ConversationId,
        text: String,
        reply_to: Option<String>,
        /// People mentioned with `@`, in the order they were picked.
        mentions: Vec<MentionRef>,
        /// Files to upload to OneDrive and share in the message.
        files: Vec<std::path::PathBuf>,
        images: Vec<crate::clipboard::InlineImage>,
    },
    Edit {
        conversation: ConversationId,
        message: String,
        text: String,
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
    /// Quotes a chat message, or opens a channel thread, in the composer.
    StartReply(String),
    StartEdit(String),
    CancelCompose,
    LoadOlder(ConversationId),
    LoadReplies {
        conversation: ConversationId,
        message: String,
    },
    /// Shows a channel thread's replies inline.
    ToggleThread(String),
    RetrySend {
        conversation: ConversationId,
        local_id: String,
    },
    /// Opens the file picker for the open conversation.
    AttachFiles,
    Paste {
        conversation: ConversationId,
        range: (usize, usize),
        text: String,
    },
    /// Opens an image already registered in the interface's local cache.
    PreviewImage(String),
    /// Adds files (picked or dropped) to the open conversation's draft.
    AddFiles(Vec<std::path::PathBuf>),
    /// Removes a file from the draft, by position.
    RemoveFile(usize),
    LoadMedia(String),

    SignIn,
    SignInWithCode,
    CancelSignIn,
    SignOut,
    /// Asks for an extra scope (incremental consent), such as channel
    /// message reading.
    RequestScope(&'static str),
    SaveAppRegistration {
        client_id: String,
        tenant: String,
    },
    /// Sets the signed-in person's availability; `None` resets it.
    SetAvailability(Option<Availability>),
    SetStatusMessage(String),
    OpenStatusMessage,
    /// Settings, on the Out of Office page, asking for the scope it needs.
    OpenOutOfOffice,
    /// Saves the automatic replies (Out of Office).
    SetOutOfOffice(OutOfOffice),
    /// Whether Chaat keeps a presence session (shows you available while it
    /// runs).
    SetPresenceSession(bool),

    SwitchAccount(usize),
    AddAccount,
    /// Abandons an account being added and goes back to the previous one.
    CancelAddAccount,

    OpenNewChat,
    /// Opens the Add people dialog for a group chat (chat id).
    OpenAddPeople(String),
    /// Adds people to a group chat; `history` shows them the earlier messages.
    AddMembers {
        chat: String,
        members: Vec<Member>,
        history: bool,
    },
    /// Opens the Leave dialog for a group chat (chat id).
    OpenLeaveChat(String),
    /// Leaves a group chat (chat id).
    LeaveChat(String),
    /// The people panel of a group chat was opened (chat id).
    PeopleOpened(String),
    /// Looks people up in the directory for the new-chat dialog or mentions.
    SearchPeople(String),
    CreateChat {
        members: Vec<Member>,
        topic: Option<String>,
    },

    JoinMeeting(String),
    StartCall {
        users: Vec<String>,
        video: bool,
    },
    /// Closes the call bar (the call itself is in Teams) and restores the
    /// window.
    CallDone,
    /// Brings the call window forward.
    ShowCall,
    /// Docks Chaat beside the call window (`true`) or restores its size.
    DockWindow(bool),
    /// Looks up the meeting's chat, asking for OnlineMeetings.Read first.
    FindMeetingChat,
    /// Shows the call's join link as a QR code, to join on a phone.
    JoinOnPhone,
    /// Opens Teams on the web once, to sign in there ahead of calls.
    PrepareTeamsWeb,
    SetCallSettings(crate::settings::CallSettings),
    OpenUrl(String),
    CopyText(String),

    OpenSwitcher,
    CloseOverlay,
    SearchMessages(String),
    /// Settings, on the page last shown.
    OpenSettings,
    /// Settings, on the keyboard shortcuts page.
    OpenShortcuts,
    ShowSettingsPage(SettingsPage),
    CloseSettings,
    SetTheme(crate::settings::ThemeChoice),
    SetNotifications(bool),
    SetKeepRunning(bool),
    /// Windows and Linux: Chaat's own title bar, or the system's frame.
    SetCustomTitlebar(bool),
    /// The push relay's URL; empty turns push off.
    SetRelay(String),
    /// The relay's access key; `None` removes it.
    SetRelayKey(Option<String>),
    /// Connect to the relay again now, for accounts whose push failed.
    RetryRelay,
    /// Look up the names Graph left out of a system event, by user id.
    LookUpNames(Vec<String>),
    TogglePerfOverlay,
    Refresh,
    DismissToast(u64),
    /// The ring toast's "Place call": call the caller back in Teams.
    RingPlaceCall(String),
    /// The ring toast's "Open chat": the chat with the caller, in Chaat.
    RingOpenChat(String),
    /// Closes a ring toast (by its call id).
    DismissRing(String),
    /// Hides the call-alerts sign-in panel; the plugin keeps waiting.
    HideCallAlertsSignIn(Option<String>),
    /// Asks the call-alerts plugin for a sign-in code for this account now.
    CallAlertsSignIn(String),
    /// Picks the call-alerts plugin's executable with the system's file picker.
    ChooseCallAlertsPlugin,
    Quit,
}

/// How a message toast looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: String,
}

/// A sign-in the call-alerts plugin is waiting for: the person enters the
/// code at the page. The plugin signs in on its own; Chaat only shows this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSignIn {
    /// The Chaat account (`oid.tid`) it is for; `None` from an older plugin.
    pub account: Option<String>,
    /// That account's sign-in name, to say whom to sign in as.
    pub username: Option<String>,
    pub verification_uri: String,
    pub user_code: String,
}
/// A ringing call the call-alerts plugin reported, as the ring toast shows it.
/// Chaat cannot answer it (calls run in Teams); the toast offers to call back
/// or open the chat. The plugin cannot tell when the call stops ringing, so the
/// toast expires on a timer.
#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    /// The plugin's call id.
    pub id: String,
    pub caller: String,
    pub video: bool,
    pub group: bool,
    /// The chat to open: the group call's thread, or the one-on-one chat with
    /// the caller, when Chaat has it.
    pub chat: Option<ConversationId>,
    /// The caller, when Chaat knows them from a chat's members. Their email
    /// address is what a call back dials.
    pub member: Option<Member>,
    /// The Chaat account (`oid.tid`) it rang on, when Chaat has that account.
    /// Its buttons switch to that account first.
    pub account: Option<String>,
    /// The account to name on the card, when that isn't obvious.
    pub account_name: Option<String>,
    /// How long the card stays: as long as the plugin said the call rings.
    pub shown_for: std::time::Duration,
    /// When the ring arrived, for the toast's expiry.
    pub arrived: std::time::Instant,
}

impl Ring {
    /// The address a call back dials, if Chaat knows it.
    pub fn email(&self) -> Option<&str> {
        self.member.as_ref().and_then(|m| m.email.as_deref())
    }

    /// Whether "Open chat" can do anything: Chaat has the chat, or knows the
    /// caller well enough to start one.
    pub fn can_open_chat(&self) -> bool {
        self.chat.is_some() || self.member.is_some()
    }
}

/// Turns Graph's legacy reaction names into emoji. Newer reactions already
/// arrive as the emoji.
pub fn reaction_emoji(reaction: &str) -> String {
    match reaction {
        "like" => "👍".into(),
        "heart" => "❤️".into(),
        "laugh" => "😆".into(),
        "surprised" => "😮".into(),
        "sad" => "😢".into(),
        "angry" => "😡".into(),
        other => other.to_owned(),
    }
}

/// The reactions offered on hover, in Teams' order.
pub const QUICK_REACTIONS: [&str; 6] = ["👍", "❤️", "😆", "😮", "😢", "😡"];

/// What the quick reactions are called, in the same order.
pub const QUICK_REACTION_NAMES: [&str; 6] = ["Like", "Heart", "Laugh", "Surprised", "Sad", "Angry"];

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, name: &str) -> Member {
        Member {
            tenant_id: None,
            user_id: id.into(),
            display_name: name.into(),
            email: None,
        }
    }

    #[test]
    fn conversation_keys_round_trip() {
        let chat = ConversationId::Chat("19:abc@thread.v2".into());
        assert_eq!(ConversationId::from_key(&chat.key()), Some(chat));
        let channel = ConversationId::Channel {
            team: "2f8e2b0c-1111-4a0a-9c55-000000000000".into(),
            channel: "19:general@thread.tacv2".into(),
        };
        assert_eq!(ConversationId::from_key(&channel.key()), Some(channel));
        assert_eq!(ConversationId::from_key("nonsense"), None);
    }

    #[test]
    fn chat_titles_name_the_other_members() {
        let mut chat = Chat {
            members: vec![member("me", "Me"), member("a", "Ada")],
            ..Default::default()
        };
        assert_eq!(chat.title("me"), "Ada");
        chat.members.extend([
            member("b", "Grace"),
            member("c", "Katherine"),
            member("d", "Dorothy"),
        ]);
        assert_eq!(chat.title("me"), "Ada, Dorothy and 2 others");
        chat.topic = Some("Launch".into());
        assert_eq!(chat.title("me"), "Launch");
    }

    #[test]
    fn chat_titles_do_not_depend_on_graphs_member_order() {
        let chat = |members| Chat {
            members,
            ..Default::default()
        };
        let one = chat(vec![
            member("me", "Me"),
            member("a", "Person A"),
            member("b", "person B"),
        ]);
        let other = chat(vec![
            member("b", "person B"),
            member("me", "Me"),
            member("a", "Person A"),
        ]);
        assert_eq!(one.title("me"), "Person A, person B");
        assert_eq!(other.title("me"), one.title("me"));
    }

    #[test]
    fn membership_changes_read_like_teams() {
        let person = |id: &str, name: Option<&str>| EventPerson {
            id: id.into(),
            name: name.map(str::to_owned),
        };
        let added = Membership {
            change: MemberChange::Added,
            by: Some(person("u-s", Some("Shiv"))),
            members: vec![person("u-g", Some("Gautam")), person("u-r", None)],
        };
        assert_eq!(
            added.describe(|id| (id == "u-r").then(|| "Rishu".to_owned())),
            "Shiv added Gautam and Rishu to the chat."
        );
        let joined = Membership {
            change: MemberChange::Joined,
            by: None,
            members: Vec::new(),
        };
        assert_eq!(joined.describe(|_| None), "People joined.");
    }

    #[test]
    fn a_one_on_one_chat_with_someone_gone_is_an_unknown_user() {
        let mut chat = Chat {
            kind: ChatKind::OneOnOne,
            members: vec![member("u-me", "Me")],
            ..Default::default()
        };
        assert_eq!(chat.title("u-me"), "Unknown user");
        chat.members.push(member("u-gone", ""));
        assert_eq!(chat.title("u-me"), "Unknown user", "kept without a name");
        chat.kind = ChatKind::Group;
        assert_eq!(chat.title("u-me"), "Just you");
    }

    #[test]
    fn the_list_orders_by_the_message_it_shows_not_chat_changes() {
        let chat = |updated: Millis, at: Millis| Chat {
            updated,
            preview: Some(Preview {
                at,
                ..Default::default()
            }),
            ..Default::default()
        };
        // Someone removed later (`updated`) must not lift an older message.
        assert_eq!(chat(9_000, 1_000).activity(), 1_000);
        let empty = Chat {
            updated: 5_000,
            ..Default::default()
        };
        assert_eq!(empty.activity(), 5_000, "a chat without messages");
    }

    #[test]
    fn unread_ignores_own_messages_and_read_marks() {
        let mut chat = Chat {
            preview: Some(Preview {
                author_id: Some("a".into()),
                at: 2_000,
                ..Default::default()
            }),
            last_read: Some(1_000),
            ..Default::default()
        };
        assert!(chat.is_unread("me"));
        chat.last_read = Some(3_000);
        assert!(!chat.is_unread("me"));
        chat.last_read = None;
        chat.preview.as_mut().unwrap().author_id = Some("me".into());
        assert!(!chat.is_unread("me"));
    }

    #[test]
    fn out_of_office_form_round_trips_and_validates() {
        let now = crate::time::from_local("2026-11-19", "10:10").unwrap();
        let off = OutOfOffice::default();
        let mut form = OutOfOfficeForm::new(&off, now);
        assert!(!form.changed_from(&off));
        assert_eq!(
            (form.start_date.as_str(), form.start_time.as_str()),
            ("2026-11-19", "10:30")
        );
        assert_eq!(
            (form.end_date.as_str(), form.end_time.as_str()),
            ("2026-11-20", "10:30")
        );

        form.replies = AutoReplies::Scheduled;
        form.internal = " Back Monday \n".into();
        assert!(form.changed_from(&off));
        let value = form.value().unwrap();
        assert_eq!(value.internal, "Back Monday");
        assert_eq!(value.end.unwrap() - value.start.unwrap(), 24 * 60 * 60_000);
        assert!(!value.active(now));
        assert!(value.active(value.start.unwrap()));
        // The saved value, with seconds and the same text, is unchanged.
        let mut saved = value.clone();
        saved.start = saved.start.map(|s| s + 7_000);
        assert!(!OutOfOfficeForm::new(&saved, now).changed_from(&saved));
        assert!(!form.changed_from(&saved));

        form.end_time = "10:00".into();
        form.end_date = form.start_date.clone();
        assert_eq!(form.value(), Err("The end must be after the start."));
        assert!(form.changed_from(&saved));
        form.end_date = "next week".into();
        assert!(form.value().unwrap_err().starts_with("Enter dates"));

        // Turning replies on ignores the unused window Exchange keeps.
        let on = OutOfOffice {
            replies: AutoReplies::On,
            ..saved.clone()
        };
        let mut form = OutOfOfficeForm::new(&on, now);
        form.start_date = "garbage".into();
        assert!(!form.changed_from(&on));
        assert!(on.active(0));
        // A schedule without a window (unreadable times) is not shown as on.
        let unknown = OutOfOffice {
            replies: AutoReplies::Scheduled,
            ..Default::default()
        };
        assert!(!unknown.active(now));
    }

    #[test]
    fn legacy_reactions_become_emoji() {
        assert_eq!(reaction_emoji("like"), "👍");
        assert_eq!(reaction_emoji("🎉"), "🎉");
    }
}
