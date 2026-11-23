//! Drafts and unsent messages that survive a restart.
//!
//! The store keeps one [`StoredDraft`] per conversation and one
//! [`OutboxEntry`] per message that was sent but not yet confirmed. After a
//! restart a young entry is sent again, after checking the conversation for a
//! copy that Graph already accepted. Text here is never logged.

use serde::{Deserialize, Serialize};

use crate::clipboard::InlineImage;
use crate::model::{ConversationId, MentionRef, Message};

/// Entries younger than this resend on start; older ones show as failed.
pub const RESEND_WINDOW_MS: i64 = 600_000;
/// A matching message may predate the entry's clock by this much (clock skew).
const DUPLICATE_SLACK_MS: i64 = 60_000;

/// What is saved of a composer: text, the quoted message and mentions. Files
/// and images stay in memory only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDraft {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub mentions: Vec<MentionRef>,
}

impl StoredDraft {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// A message to send: what the backend needs to deliver it. Also the part of
/// an [`OutboxEntry`] that is saved to disk (flattened, so the file keeps its
/// field names).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendPayload {
    pub conversation: ConversationId,
    pub local_id: String,
    pub html: String,
    /// A chat message to quote, or a channel post to reply under.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// The people `<at id="n">` tags in `html` name, by index.
    #[serde(default)]
    pub mentions: Vec<MentionRef>,
    /// Files to upload to OneDrive and share in the message.
    #[serde(default)]
    pub files: Vec<std::path::PathBuf>,
    #[serde(default)]
    pub images: Vec<InlineImage>,
    /// Who to share the files with (the chat's members' addresses).
    #[serde(default)]
    pub recipients: Vec<String>,
}

/// A message handed to the backend that Graph has not confirmed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    #[serde(flatten)]
    pub payload: SendPayload,
    pub created: i64,
    /// The plain text, for the duplicate check and the failed row.
    pub text: String,
}

impl OutboxEntry {
    /// Whether the entry is young enough to send again on its own.
    pub fn resendable(&self, now: i64) -> bool {
        now - self.created < RESEND_WINDOW_MS
    }
}

/// The message in `recent` that is this entry already delivered, if any: mine,
/// with the same trimmed text, created no earlier than shortly before the
/// entry.
pub fn already_sent(entry: &OutboxEntry, recent: &[Message], me: &str) -> Option<Message> {
    let wanted = entry.text.trim();
    recent
        .iter()
        .find(|m| {
            m.is_from(me)
                && !m.deleted
                && !m.system
                && m.created >= entry.created - DUPLICATE_SLACK_MS
                && m.body.to_plain().trim() == wanted
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::RichText;

    fn entry() -> OutboxEntry {
        OutboxEntry {
            payload: SendPayload {
                local_id: "local-3".into(),
                conversation: ConversationId::Chat("19:a".into()),
                html: "<p>hi</p>".into(),
                reply_to: None,
                mentions: vec![],
                files: vec![],
                images: vec![],
                recipients: vec![],
            },
            created: 1_700_000_000_000,
            text: "hi".into(),
        }
    }

    fn mine(text: &str, created: i64) -> Message {
        Message {
            id: "m1".into(),
            author_id: Some("me".into()),
            created,
            body: RichText::plain(text),
            ..Message::default()
        }
    }

    #[test]
    fn finds_a_delivered_copy() {
        let e = entry();
        let found = already_sent(&e, &[mine(" hi ", e.created + 2_000)], "me");
        assert_eq!(found.map(|m| m.id), Some("m1".to_owned()));
    }

    #[test]
    fn dup_window() {
        let e = entry();
        assert!(already_sent(&e, &[mine("hi", e.created - 60_001)], "me").is_none());
        assert!(already_sent(&e, &[mine("hi", e.created - 60_000)], "me").is_some());
    }

    #[test]
    fn other_author_or_text_is_not_a_duplicate() {
        let e = entry();
        assert!(already_sent(&e, &[mine("hi", e.created)], "you").is_none());
        assert!(already_sent(&e, &[mine("hello", e.created)], "me").is_none());
    }

    #[test]
    fn resend_window() {
        let e = entry();
        assert!(e.resendable(e.created + 599_999));
        assert!(!e.resendable(e.created + 600_000));
    }

    #[test]
    fn outbox_json_unchanged() {
        // An entry as it is written today, every field present.
        let written = serde_json::json!({
            "local_id": "local-3",
            "conversation": {"Channel": {"team": "t1", "channel": "c1"}},
            "created": 1_700_000_000_000_i64,
            "html": "<p>hi @Ada</p>",
            "text": "hi @Ada",
            "reply_to": "m9",
            "mentions": [{"user_id": "u-ada", "name": "Ada"}],
            "files": ["C:/x/y.txt"],
            "images": [{"png": "AQID", "width": 1, "height": 2, "uri": "cid:img1"}],
            "recipients": ["ada@example.com"]
        });
        let entry: OutboxEntry = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(entry.payload.reply_to.as_deref(), Some("m9"));
        assert_eq!(entry.payload.files.len(), 1);
        assert_eq!(serde_json::to_value(&entry).unwrap(), written);
        let text = serde_json::to_string(&entry).unwrap();
        let again: OutboxEntry = serde_json::from_str(&text).unwrap();
        assert_eq!(again, entry);
    }

    #[test]
    fn outbox_missing_optional_fields_loads() {
        let written = serde_json::json!({
            "local_id": "local-4",
            "conversation": {"Chat": "19:a"},
            "created": 5,
            "html": "<p>x</p>",
            "text": "x"
        });
        let entry: OutboxEntry = serde_json::from_value(written).unwrap();
        assert_eq!(entry.payload.reply_to, None);
        assert!(entry.payload.mentions.is_empty());
        assert!(entry.payload.files.is_empty());
        assert!(entry.payload.images.is_empty());
        assert!(entry.payload.recipients.is_empty());
    }
}
