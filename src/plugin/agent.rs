//! The agent protocol: an agent plugin (capability `agents`) asks Chaat to
//! read cached messages and proposes replies and reactions. Everything here
//! is untrusted plugin input: strings are capped and cleaned, limits are
//! clamped, and anything malformed is dropped. Chaat decides what is allowed
//! (`agents::access`) and the person approves every proposal.

use serde_json::{Value, json};

use super::protocol::{HostMessage, is_invisible, plain_text};

/// The most characters in a conversation or message id.
const MAX_ID: usize = 512;
/// The most characters in a search query.
const MAX_QUERY: usize = 2000;
/// The most characters in a proposed reply.
const MAX_TEXT: usize = 20_000;
/// The most characters in an agent's name.
const MAX_AGENT: usize = 40;
/// The most characters in a reaction emoji.
const MAX_EMOJI: usize = 16;

/// A read request from an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentQuery {
    ListChats {
        limit: usize,
    },
    ListChannels,
    ReadMessages {
        /// A `ConversationId::key()` string.
        conversation: String,
        before: Option<i64>,
        limit: usize,
    },
    /// Messages since a time; `since` 0 means the caller decides.
    Recent {
        since: i64,
        limit: usize,
        unread_only: bool,
    },
    Search {
        query: String,
        limit: usize,
    },
}

/// A reply or reaction an agent wants the person to approve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentProposal {
    Reply {
        conversation: String,
        text: String,
        reply_to: Option<String>,
        agent: String,
    },
    Reaction {
        conversation: String,
        message: String,
        emoji: String,
        agent: String,
    },
}

/// A limit field: `default` when absent or not a number, else clamped to
/// `1..=max`.
fn limit(value: &Value, default: usize, max: usize) -> usize {
    match value.get("limit") {
        Some(Value::Number(n)) => {
            let n = n
                .as_i64()
                .or_else(|| n.as_f64().map(|f| f as i64))
                .unwrap_or(1);
            usize::try_from(n.max(1)).unwrap_or(max).min(max)
        }
        _ => default,
    }
}

/// A string field of 1 to `max` characters.
fn id_field(value: &Value, key: &str, max: usize) -> Option<String> {
    let text = value.get(key)?.as_str()?;
    let count = text.chars().count();
    (1..=max).contains(&count).then(|| text.to_owned())
}

fn number(value: &Value, key: &str) -> Option<i64> {
    match value.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

/// The query in an `agent_query` message, or `None` when it is of an unknown
/// kind or malformed.
pub fn parse_query(value: &Value) -> Option<AgentQuery> {
    match value.get("kind")?.as_str()? {
        "list_chats" => Some(AgentQuery::ListChats {
            limit: limit(value, 50, 200),
        }),
        "list_channels" => Some(AgentQuery::ListChannels),
        "read_messages" => Some(AgentQuery::ReadMessages {
            conversation: id_field(value, "conversation", MAX_ID)?,
            before: number(value, "before"),
            limit: limit(value, 30, 200),
        }),
        "recent_messages" => Some(AgentQuery::Recent {
            since: number(value, "since").unwrap_or(0),
            limit: limit(value, 50, 200),
            unread_only: value.get("unread_only") == Some(&Value::Bool(true)),
        }),
        "search" => Some(AgentQuery::Search {
            query: id_field(value, "query", MAX_QUERY)?,
            limit: limit(value, 20, 100),
        }),
        _ => None,
    }
}

/// Proposed text without control or invisible characters, newlines kept.
/// `None` when it is empty after cleaning or longer than 20 000 characters.
fn clean_text(text: &str) -> Option<String> {
    let cleaned: String = text
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| *c == '\n' || !(c.is_control() || is_invisible(*c)))
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned.chars().count() > MAX_TEXT {
        return None;
    }
    Some(cleaned.to_owned())
}

fn agent_name(value: &Value) -> String {
    value
        .get("agent")
        .and_then(Value::as_str)
        .and_then(|name| plain_text(name, MAX_AGENT))
        .unwrap_or_else(|| "Agent".to_owned())
}

/// The proposal in an `agent_propose` message, or `None` when it is of an
/// unknown kind or malformed.
pub fn parse_proposal(value: &Value) -> Option<AgentProposal> {
    let conversation = id_field(value, "conversation", MAX_ID)?;
    match value.get("kind")?.as_str()? {
        "reply" => Some(AgentProposal::Reply {
            conversation,
            text: clean_text(value.get("text")?.as_str()?)?,
            reply_to: id_field(value, "reply_to", MAX_ID),
            agent: agent_name(value),
        }),
        "reaction" => {
            let emoji = value.get("emoji")?.as_str()?;
            let count = emoji.chars().count();
            if !(1..=MAX_EMOJI).contains(&count)
                || emoji.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return None;
            }
            Some(AgentProposal::Reaction {
                conversation,
                message: id_field(value, "message", MAX_ID)?,
                emoji: emoji.to_owned(),
                agent: agent_name(value),
            })
        }
        _ => None,
    }
}

/// The answer to request `request`: its data, or a fixed error code
/// (`not_allowed`, `not_found`, `bad_request`, `no_account`, `busy`).
pub fn result_line(request: u64, data: Result<Value, &'static str>) -> HostMessage {
    match data {
        Ok(data) => HostMessage::AgentResult {
            id: request,
            ok: true,
            data,
        },
        Err(code) => HostMessage::AgentResult {
            id: request,
            ok: false,
            data: json!({ "error": code }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_parse_with_defaults() {
        assert_eq!(
            parse_query(
                &json!({"kind":"read_messages","conversation":"chat:19:a@thread.v2","limit":30})
            ),
            Some(AgentQuery::ReadMessages {
                conversation: "chat:19:a@thread.v2".into(),
                before: None,
                limit: 30
            })
        );
        assert_eq!(
            parse_query(&json!({"kind":"list_chats"})),
            Some(AgentQuery::ListChats { limit: 50 })
        );
        assert_eq!(
            parse_query(&json!({"kind":"list_channels"})),
            Some(AgentQuery::ListChannels)
        );
        assert_eq!(
            parse_query(&json!({"kind":"recent_messages"})),
            Some(AgentQuery::Recent {
                since: 0,
                limit: 50,
                unread_only: false
            })
        );
        assert_eq!(
            parse_query(&json!({"kind":"search","query":"lunch"})),
            Some(AgentQuery::Search {
                query: "lunch".into(),
                limit: 20
            })
        );
        assert_eq!(
            parse_query(&json!({"kind":"read_messages","conversation":"c","before":5})),
            Some(AgentQuery::ReadMessages {
                conversation: "c".into(),
                before: Some(5),
                limit: 30
            })
        );
    }

    #[test]
    fn unknown_query_ignored() {
        assert_eq!(parse_query(&json!({"kind":"nope"})), None);
        assert_eq!(parse_query(&json!({"kind":"drop_table"})), None);
        assert_eq!(parse_query(&json!({})), None);
        assert_eq!(parse_query(&json!({"kind":5})), None);
        assert_eq!(parse_query(&json!({"kind":"read_messages"})), None);
        assert_eq!(parse_query(&json!({"kind":"search","query":""})), None);
    }

    #[test]
    fn limits_clamped() {
        let chats = |limit: Value| parse_query(&json!({"kind":"list_chats","limit":limit}));
        assert_eq!(chats(json!(0)), Some(AgentQuery::ListChats { limit: 1 }));
        assert_eq!(chats(json!(-4)), Some(AgentQuery::ListChats { limit: 1 }));
        assert_eq!(
            chats(json!(900)),
            Some(AgentQuery::ListChats { limit: 200 })
        );
        assert_eq!(chats(json!("x")), Some(AgentQuery::ListChats { limit: 50 }));
        assert_eq!(
            parse_query(&json!({"kind":"search","query":"a","limit":900})),
            Some(AgentQuery::Search {
                query: "a".into(),
                limit: 100
            })
        );
    }

    #[test]
    fn long_strings_are_refused() {
        let long_id = "x".repeat(513);
        assert_eq!(
            parse_query(&json!({"kind":"read_messages","conversation":long_id})),
            None
        );
        assert_eq!(
            parse_query(&json!({"kind":"search","query":"q".repeat(2001)})),
            None
        );
        assert!(parse_query(&json!({"kind":"search","query":"q".repeat(2000)})).is_some());
    }

    #[test]
    fn proposals_parse() {
        assert_eq!(
            parse_proposal(
                &json!({"kind":"reply","conversation":"chat:x","text":"On it","agent":"Claude Code"})
            ),
            Some(AgentProposal::Reply {
                conversation: "chat:x".into(),
                text: "On it".into(),
                reply_to: None,
                agent: "Claude Code".into()
            })
        );
        assert_eq!(
            parse_proposal(
                &json!({"kind":"reaction","conversation":"chat:x","message":"m1","emoji":"👍","agent":"A"})
            ),
            Some(AgentProposal::Reaction {
                conversation: "chat:x".into(),
                message: "m1".into(),
                emoji: "👍".into(),
                agent: "A".into()
            })
        );
        assert_eq!(
            parse_proposal(&json!({"kind":"edit","conversation":"c"})),
            None
        );
    }

    #[test]
    fn agent_text_is_cleaned() {
        let proposal =
            |text: &str| parse_proposal(&json!({"kind":"reply","conversation":"c","text":text}));
        assert_eq!(
            proposal("hi\u{0007}\u{202E} there\r\nline two\u{200B}"),
            Some(AgentProposal::Reply {
                conversation: "c".into(),
                text: "hi there\nline two".into(),
                reply_to: None,
                agent: "Agent".into()
            })
        );
        assert_eq!(proposal("\u{0007} \u{200B}\n"), None);
        assert_eq!(proposal(&"a".repeat(20_001)), None);
        assert!(proposal(&"a".repeat(20_000)).is_some());
    }

    #[test]
    fn agent_name_short() {
        let Some(AgentProposal::Reply { agent, .. }) = parse_proposal(
            &json!({"kind":"reply","conversation":"c","text":"t","agent":"N".repeat(100)}),
        ) else {
            panic!("expected a reply");
        };
        assert_eq!(agent.chars().count(), 40);
        let Some(AgentProposal::Reply { agent, .. }) = parse_proposal(
            &json!({"kind":"reply","conversation":"c","text":"t","agent":"\u{0007}\n"}),
        ) else {
            panic!("expected a reply");
        };
        assert_eq!(agent, "Agent");
    }

    #[test]
    fn emoji_is_short_and_has_no_whitespace() {
        let reaction = |emoji: &str| {
            parse_proposal(
                &json!({"kind":"reaction","conversation":"c","message":"m","emoji":emoji}),
            )
        };
        assert!(reaction("👍").is_some());
        assert!(reaction("").is_none());
        assert!(reaction("a b").is_none());
        assert!(reaction(&"e".repeat(17)).is_none());
        assert!(reaction(&"e".repeat(16)).is_some());
    }

    #[test]
    fn errors_become_failed_results() {
        assert_eq!(
            result_line(7, Err("not_allowed")),
            HostMessage::AgentResult {
                id: 7,
                ok: false,
                data: json!({"error":"not_allowed"})
            }
        );
        assert_eq!(
            result_line(8, Ok(json!([1]))),
            HostMessage::AgentResult {
                id: 8,
                ok: true,
                data: json!([1])
            }
        );
    }
}
