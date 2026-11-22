//! The plain view of messages that agents get, and fitting it into a line.

use serde_json::{Value, json};

use crate::model::{AttachmentKind, Message};
use crate::time;

/// Room kept for the envelope around the items (`agent_result`, cursors).
const OVERHEAD: usize = 2048;

/// One message as an agent sees it: text only, no HTML, no URLs, no ids of
/// other people beyond the sender's. `None` for deleted messages and system
/// events.
pub fn message(message: &Message) -> Option<Value> {
    if message.deleted || message.system {
        return None;
    }
    let reactions: Vec<Value> = message
        .reactions
        .iter()
        .map(|r| json!({"emoji": r.emoji, "count": r.users.len()}))
        .collect();
    let mut view = json!({
        "message_id": message.id,
        "sender": message.author,
        "sender_id": message.author_id,
        "time": format!("{}Z", time::to_utc_civil(message.created)),
        "text": message.body.to_plain(),
        "reply_to": message.reply_to,
        "reactions": reactions,
    });
    let object = view.as_object_mut()?;
    if let Some(subject) = &message.subject {
        object.insert("subject".into(), json!(subject));
    }
    if message.edited {
        object.insert("edited".into(), json!(true));
    }
    let files: Vec<&str> = message
        .attachments
        .iter()
        .filter(|a| matches!(a.kind, AttachmentKind::File))
        .filter_map(|a| a.name.as_deref())
        .collect();
    if !files.is_empty() {
        object.insert("files".into(), json!(files));
    }
    Some(view)
}

fn size(value: &Value) -> usize {
    serde_json::to_string(value).map_or(0, |s| s.len())
}

/// Keeps the newest items that fit `max_bytes` with 2 KiB to spare. `items`
/// is oldest first and so is the result. The flag says something was dropped
/// or cut. A single item over the budget keeps a cut of its text ending in
/// an ellipsis.
pub fn fit(items: Vec<Value>, max_bytes: usize) -> (Vec<Value>, bool) {
    let budget = max_bytes.saturating_sub(OVERHEAD);
    let total = items.len();
    let mut kept: Vec<Value> = Vec::new();
    let mut used = 2; // the brackets
    let mut shortened = false;
    for item in items.into_iter().rev() {
        let cost = size(&item) + 1; // the comma
        if used + cost <= budget {
            used += cost;
            kept.push(item);
            continue;
        }
        if kept.is_empty() {
            kept.push(cut(item, budget.saturating_sub(2)));
            shortened = true;
        }
        break;
    }
    kept.reverse();
    let dropped = shortened || kept.len() < total;
    (kept, dropped)
}

/// Shortens `text` until the item serializes within `budget` bytes.
fn cut(mut item: Value, budget: usize) -> Value {
    let Some(text) = item.get("text").and_then(Value::as_str).map(str::to_owned) else {
        return item;
    };
    let chars: Vec<char> = text.chars().collect();
    let with = |n: usize, item: &mut Value| {
        let mut shown: String = chars[..n].iter().collect();
        shown.push('…');
        item["text"] = json!(shown);
    };
    // The largest prefix that fits; size grows with the prefix.
    let (mut low, mut high) = (0, chars.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        with(mid, &mut item);
        if size(&item) <= budget {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    with(low, &mut item);
    item
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::RichText;
    use crate::model::{Attachment, Reaction};

    fn sample(id: &str, text: &str) -> Message {
        Message {
            id: id.into(),
            author: "Ada".into(),
            body: RichText::plain(text),
            ..Default::default()
        }
    }

    #[test]
    fn examples() {
        let mut deleted = sample("m", "x");
        deleted.deleted = true;
        assert!(message(&deleted).is_none());
        let mut system = sample("m", "x");
        system.system = true;
        assert!(message(&system).is_none());

        assert_eq!(
            message(&sample("m1", "Hi")).unwrap(),
            json!({"message_id":"m1","sender":"Ada","sender_id":null,
                   "time":"1970-01-01T00:00:00Z","text":"Hi","reply_to":null,"reactions":[]})
        );
    }

    #[test]
    fn optional_fields_and_html_free_text() {
        let mut m = sample("m1", "");
        m.body = crate::html::parse("<p>One</p><p>Two <b>bold</b></p>");
        m.subject = Some("Topic".into());
        m.edited = true;
        m.reply_to = Some("root".into());
        m.reactions = vec![Reaction {
            emoji: "👍".into(),
            users: vec!["a".into(), "b".into()],
        }];
        m.attachments = vec![
            Attachment {
                id: "1".into(),
                kind: AttachmentKind::File,
                name: Some("plan.pdf".into()),
                content_type: None,
                url: Some("https://example.invalid/x".into()),
                content: None,
            },
            Attachment {
                id: "2".into(),
                kind: AttachmentKind::Card,
                name: Some("card".into()),
                content_type: None,
                url: None,
                content: None,
            },
        ];
        let view = message(&m).unwrap();
        assert_eq!(view["text"], "One\nTwo bold");
        assert_eq!(view["subject"], "Topic");
        assert_eq!(view["edited"], true);
        assert_eq!(view["reply_to"], "root");
        assert_eq!(view["reactions"], json!([{"emoji":"👍","count":2}]));
        assert_eq!(view["files"], json!(["plan.pdf"]));
        assert!(!view.to_string().contains("example.invalid"));
    }

    fn item(id: &str, bytes: usize) -> Value {
        json!({"message_id": id, "text": "x".repeat(bytes)})
    }

    #[test]
    fn fit_keeps_newest_side() {
        let items = vec![item("a", 30_000), item("b", 30_000), item("c", 30_000)];
        let (kept, dropped) = fit(items, 60_000);
        assert!(dropped);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["message_id"], "c");

        let items = vec![item("a", 10), item("b", 10)];
        let (kept, dropped) = fit(items, 60_000);
        assert!(!dropped);
        assert_eq!(kept[0]["message_id"], "a");
        assert_eq!(kept[1]["message_id"], "b");
    }

    #[test]
    fn huge_single_message() {
        let (kept, dropped) = fit(vec![item("a", 90_000)], 60_000);
        assert!(dropped);
        assert_eq!(kept.len(), 1);
        assert!(serde_json::to_string(&kept).unwrap().len() <= 60_000 - OVERHEAD);
        let text = kept[0]["text"].as_str().unwrap();
        assert!(text.ends_with('…'));
        assert!(text.len() > 50_000);
    }

    #[test]
    fn huge_multibyte_message_cuts_on_a_char() {
        let big = json!({"message_id": "a", "text": "é".repeat(60_000)});
        let (kept, dropped) = fit(vec![big], 20_000);
        assert!(dropped);
        assert!(serde_json::to_string(&kept).unwrap().len() <= 20_000);
        assert!(kept[0]["text"].as_str().unwrap().ends_with('…'));
    }
}
