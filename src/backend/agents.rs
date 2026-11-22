//! Answers agent reads from the cache. Nothing here reaches Microsoft: no
//! Graph request, no fetch, no freshness claim. An agent sees what Chaat
//! already holds, and only in the conversations the person allowed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::agents::{access::AgentAccess, view};
use crate::model::{Chat, ChatKind, ConversationId, Message};
use crate::plugin::agent::AgentQuery;
use crate::store::Store;
use crate::time;

/// The most bytes of one answer; `view::fit` keeps room for the envelope.
pub const MAX_ANSWER: usize = 60_000;
/// The most reads running at once; more answer `busy`.
const MAX_IN_FLIGHT: usize = 8;
/// Rows asked of the store per row kept, before the access filter.
const OVER_FETCH: usize = 5;
const MAX_ROWS: usize = 1000;

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

struct Slot;

impl Slot {
    fn take() -> Option<Self> {
        if IN_FLIGHT.fetch_add(1, Ordering::AcqRel) >= MAX_IN_FLIGHT {
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Runs one read on the blocking pool. `me` is the account's user id.
pub async fn answer(
    store: Arc<Mutex<Store>>,
    me: String,
    query: AgentQuery,
    access: AgentAccess,
) -> Result<Value, &'static str> {
    let Some(slot) = Slot::take() else {
        return Err("busy");
    };
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        read(&store, &me, &query, &access)
    })
    .await
    .unwrap_or(Err("busy"))
}

/// Answers `query` from `store`, filtered again by `access`.
pub fn read(
    store: &Store,
    me: &str,
    query: &AgentQuery,
    access: &AgentAccess,
) -> Result<Value, &'static str> {
    let names = Names::load(store, me);
    match query {
        AgentQuery::ListChats { limit } => Ok(list_chats(&names, access, *limit)),
        AgentQuery::ListChannels => Ok(list_channels(store, &names, access)),
        AgentQuery::ReadMessages {
            conversation,
            before,
            limit,
        } => read_messages(store, &names, access, conversation, *before, *limit),
        AgentQuery::Recent {
            since,
            limit,
            unread_only,
        } => Ok(recent(store, &names, access, *since, *limit, *unread_only)),
        AgentQuery::Search { query, limit } => Ok(search(store, &names, access, query, *limit)),
    }
}

/// Titles and unread state of what the cache knows.
struct Names {
    me: String,
    chats: HashMap<String, Chat>,
    /// Channel conversation key to (team name, channel name).
    channels: HashMap<String, (String, String)>,
}

impl Names {
    fn load(store: &Store, me: &str) -> Self {
        let chats = store
            .chats()
            .unwrap_or_default()
            .into_iter()
            .map(|chat| (chat.id.clone(), chat))
            .collect();
        let teams: HashMap<String, String> = store
            .teams()
            .unwrap_or_default()
            .into_iter()
            .map(|team| (team.id, team.name))
            .collect();
        let channels = store
            .channels()
            .unwrap_or_default()
            .into_iter()
            .map(|channel| {
                let key = ConversationId::Channel {
                    team: channel.team_id.clone(),
                    channel: channel.id.clone(),
                }
                .key();
                let team = teams.get(&channel.team_id).cloned().unwrap_or_default();
                (key, (team, channel.name))
            })
            .collect();
        Self {
            me: me.to_owned(),
            chats,
            channels,
        }
    }

    fn known(&self, conversation: &ConversationId) -> bool {
        match conversation {
            ConversationId::Chat(id) => self.chats.contains_key(id),
            ConversationId::Channel { .. } => self.channels.contains_key(&conversation.key()),
        }
    }

    fn title(&self, conversation: &ConversationId) -> String {
        match conversation {
            ConversationId::Chat(id) => self
                .chats
                .get(id)
                .map(|chat| chat.title(&self.me))
                .unwrap_or_default(),
            ConversationId::Channel { .. } => self
                .channels
                .get(&conversation.key())
                .map(|(team, channel)| format!("{team} / {channel}"))
                .unwrap_or_default(),
        }
    }

    fn unread(&self, conversation: &ConversationId) -> bool {
        match conversation {
            ConversationId::Chat(id) => self.chats.get(id).is_some_and(|c| c.is_unread(&self.me)),
            ConversationId::Channel { .. } => false,
        }
    }
}

fn clamp(limit: usize) -> usize {
    limit.clamp(1, 200)
}

/// Fits a newest-first list: keeps the newest items that fit.
fn fit_newest_first(mut items: Vec<Value>) -> (Vec<Value>, bool) {
    items.reverse();
    let (mut kept, cut) = view::fit(items, MAX_ANSWER);
    kept.reverse();
    (kept, cut)
}

fn kind_name(kind: ChatKind) -> &'static str {
    match kind {
        ChatKind::OneOnOne => "one_on_one",
        ChatKind::Group => "group",
        ChatKind::Meeting => "meeting",
    }
}

fn list_chats(names: &Names, access: &AgentAccess, limit: usize) -> Value {
    let mut chats: Vec<&Chat> = names
        .chats
        .values()
        .filter(|chat| !chat.hidden)
        .filter(|chat| access.can_read(&ConversationId::Chat(chat.id.clone())))
        .collect();
    chats.sort_by_key(|chat| (std::cmp::Reverse(chat.activity()), chat.id.clone()));
    let items: Vec<Value> = chats
        .into_iter()
        .take(clamp(limit))
        .map(|chat| {
            json!({
                "id": ConversationId::Chat(chat.id.clone()).key(),
                "title": chat.title(&names.me),
                "kind": kind_name(chat.kind),
                "last_message_at": chat.preview.as_ref().map(|p| time::to_iso(p.at)),
                "unread": chat.is_unread(&names.me),
            })
        })
        .collect();
    let (chats, _) = fit_newest_first(items);
    json!({ "chats": chats })
}

fn list_channels(store: &Store, names: &Names, access: &AgentAccess) -> Value {
    let mut rows: Vec<(String, String, String, Option<i64>)> = Vec::new();
    for channel in store.channels().unwrap_or_default() {
        let id = ConversationId::Channel {
            team: channel.team_id.clone(),
            channel: channel.id.clone(),
        };
        if !access.can_read(&id) {
            continue;
        }
        let (team, name) = names.channels.get(&id.key()).cloned().unwrap_or_default();
        let last = store
            .messages(&id, 1)
            .ok()
            .and_then(|m| m.last().map(|m| m.created));
        rows.push((id.key(), team, name, last));
    }
    rows.sort_by_key(|(key, _, _, last)| (std::cmp::Reverse(*last), key.clone()));
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(key, team, channel, last)| {
            let mut item = json!({"id": key, "team": team, "channel": channel});
            if let Some(last) = last {
                item["last_message_at"] = json!(time::to_iso(last));
            }
            item
        })
        .collect();
    let (channels, _) = fit_newest_first(items);
    json!({ "channels": channels })
}

fn read_messages(
    store: &Store,
    names: &Names,
    access: &AgentAccess,
    key: &str,
    before: Option<i64>,
    limit: usize,
) -> Result<Value, &'static str> {
    let id = ConversationId::from_key(key).ok_or("bad_request")?;
    if !access.can_read(&id) {
        return Err("not_allowed");
    }
    let limit = clamp(limit);
    // One more than asked, to know whether older ones exist.
    let mut messages = match before {
        Some(before) => store.messages_before(&id, before, limit + 1),
        None => store.messages(&id, limit + 1),
    }
    .map_err(|_| "busy")?;
    if messages.is_empty() && !names.known(&id) {
        return Err("not_found");
    }
    let older = messages.len() > limit;
    if older {
        messages.remove(0);
    }
    let pairs: Vec<(i64, Value)> = messages
        .iter()
        .filter_map(|m| view::message(m).map(|v| (m.created, v)))
        .collect();
    let views: Vec<Value> = pairs.iter().map(|(_, v)| v.clone()).collect();
    let (kept, cut) = view::fit(views, MAX_ANSWER);
    let oldest = pairs.len().saturating_sub(kept.len());
    let more = older || cut;
    let mut answer = json!({
        "conversation_id": id.key(),
        "messages": kept,
        "more": more,
    });
    if more && let Some((created, _)) = pairs.get(oldest) {
        answer["before"] = json!(created);
    }
    Ok(answer)
}

fn recent(
    store: &Store,
    names: &Names,
    access: &AgentAccess,
    since: i64,
    limit: usize,
    unread_only: bool,
) -> Value {
    let limit = clamp(limit);
    let rows = (limit * OVER_FETCH).min(MAX_ROWS);
    let messages: Vec<Message> = store.recent_messages(since, rows).unwrap_or_default();
    let mut items: Vec<Value> = Vec::new();
    let mut more = false;
    for message in &messages {
        let Some(id) = message.conversation.as_ref() else {
            continue;
        };
        if !access.can_read(id) || (unread_only && !names.unread(id)) {
            continue;
        }
        let Some(mut item) = view::message(message) else {
            continue;
        };
        if items.len() == limit {
            more = true;
            break;
        }
        item["conversation_id"] = json!(id.key());
        item["conversation"] = json!(names.title(id));
        items.push(item);
    }
    let (messages, cut) = fit_newest_first(items);
    json!({ "messages": messages, "more": more || cut })
}

fn search(store: &Store, names: &Names, access: &AgentAccess, query: &str, limit: usize) -> Value {
    let limit = clamp(limit);
    let rows = (limit * OVER_FETCH).min(MAX_ROWS);
    let hits = store.search(query, rows).unwrap_or_default();
    let items: Vec<Value> = hits
        .into_iter()
        .filter(|hit| access.can_read(&hit.conversation))
        .take(limit)
        .map(|hit| {
            json!({
                "conversation_id": hit.conversation.key(),
                "conversation": names.title(&hit.conversation),
                "message_id": hit.message_id,
                "sender": hit.author,
                "time": time::to_iso(hit.at),
                "snippet": hit.snippet,
            })
        })
        .collect();
    let (hits, _) = fit_newest_first(items);
    json!({ "hits": hits })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::access::Access;
    use crate::html::RichText;
    use crate::model::{Channel, Member, Preview, Team};
    use std::collections::BTreeSet;

    const ME: &str = "u-me";

    fn member(id: &str, name: &str) -> Member {
        Member {
            user_id: id.into(),
            display_name: name.into(),
            ..Default::default()
        }
    }

    fn chat(id: &str, kind: ChatKind, topic: Option<&str>, at: i64, hidden: bool) -> Chat {
        Chat {
            id: id.into(),
            kind,
            topic: topic.map(str::to_owned),
            members: vec![member(ME, "Alex"), member("u-ada", "Ada Lovelace")],
            preview: Some(Preview {
                message_id: "m".into(),
                author_id: Some("u-ada".into()),
                author: Some("Ada Lovelace".into()),
                text: "hi".into(),
                at,
                deleted: false,
                ..Default::default()
            }),
            last_read: (id != "unread").then_some(at),
            updated: at,
            hidden,
            ..Default::default()
        }
    }

    fn message(conversation: &ConversationId, id: &str, created: i64, text: &str) -> Message {
        Message {
            id: id.into(),
            conversation: Some(conversation.clone()),
            author: "Ada Lovelace".into(),
            author_id: Some("u-ada".into()),
            created,
            modified: created,
            body: RichText::plain(text),
            ..Default::default()
        }
    }

    fn open(chats: &[Chat]) -> Store {
        let mut store = Store::in_memory().unwrap();
        store.save_chats(chats).unwrap();
        store
    }

    fn all() -> AgentAccess {
        AgentAccess {
            chats: Access::All,
            channels: Access::All,
            propose: false,
        }
    }

    fn only(keys: &[&str]) -> AgentAccess {
        let keys: BTreeSet<String> = keys.iter().map(|k| (*k).to_owned()).collect();
        AgentAccess {
            chats: Access::Selected(keys),
            ..Default::default()
        }
    }

    fn run(store: &Store, query: AgentQuery, access: &AgentAccess) -> Result<Value, &'static str> {
        read(store, ME, &query, access)
    }

    fn seeded() -> Store {
        let mut store = open(&[
            chat("a", ChatKind::OneOnOne, None, 3_000, false),
            chat("b", ChatKind::Group, Some("Launch"), 2_000, false),
            chat("unread", ChatKind::Meeting, Some("Standup"), 4_000, false),
            chat("gone", ChatKind::Group, Some("Hidden"), 5_000, true),
        ]);
        for (id, text, at) in [
            ("a", "lunch plans", 3_000),
            ("b", "lunch menu", 2_000),
            ("unread", "lunch today", 4_000),
        ] {
            let conversation = ConversationId::Chat(id.into());
            store
                .save_messages(&conversation, &[message(&conversation, "m1", at, text)])
                .unwrap();
        }
        store
    }

    fn ids(answer: &Value, list: &str, field: &str) -> Vec<String> {
        answer[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item[field].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn list_chats_titles() {
        let store = seeded();
        let answer = run(&store, AgentQuery::ListChats { limit: 50 }, &all()).unwrap();
        // Newest activity first; the hidden chat is skipped.
        assert_eq!(
            ids(&answer, "chats", "id"),
            ["chat:unread", "chat:a", "chat:b"]
        );
        let chats = answer["chats"].as_array().unwrap();
        assert_eq!(chats[1]["title"], "Ada Lovelace");
        assert_eq!(chats[1]["kind"], "one_on_one");
        assert_eq!(chats[2]["title"], "Launch");
        assert_eq!(chats[2]["kind"], "group");
        assert_eq!(chats[0]["kind"], "meeting");
        assert_eq!(chats[0]["unread"], true);
        assert_eq!(chats[1]["unread"], false);
        assert_eq!(chats[1]["last_message_at"], "1970-01-01T00:00:03.000Z");
    }

    #[test]
    fn list_chats_applies_limit_and_access() {
        let store = seeded();
        let answer = run(&store, AgentQuery::ListChats { limit: 2 }, &all()).unwrap();
        assert_eq!(answer["chats"].as_array().unwrap().len(), 2);
        let list = AgentQuery::ListChats { limit: 50 };
        let answer = run(&store, list.clone(), &only(&["chat:b"])).unwrap();
        assert_eq!(ids(&answer, "chats", "id"), ["chat:b"]);
        let answer = run(&store, list, &AgentAccess::default()).unwrap();
        assert!(answer["chats"].as_array().unwrap().is_empty());
    }

    #[test]
    fn filters_again_in_worker() {
        let store = seeded();
        let access = only(&["chat:a"]);
        let search = AgentQuery::Search {
            query: "lunch".into(),
            limit: 50,
        };
        let hits = run(&store, search, &access).unwrap();
        assert_eq!(ids(&hits, "hits", "conversation_id"), ["chat:a"]);
        let hit = &hits["hits"][0];
        assert_eq!(hit["conversation"], "Ada Lovelace");
        assert_eq!(hit["sender"], "Ada Lovelace");
        assert_eq!(hit["message_id"], "m1");

        let recent = AgentQuery::Recent {
            since: 0,
            limit: 50,
            unread_only: false,
        };
        let messages = run(&store, recent, &access).unwrap();
        assert_eq!(ids(&messages, "messages", "conversation_id"), ["chat:a"]);

        let read = AgentQuery::ReadMessages {
            conversation: "chat:b".into(),
            before: None,
            limit: 10,
        };
        assert_eq!(run(&store, read, &access), Err("not_allowed"));
    }

    #[test]
    fn recent_newest_first_and_unread_only() {
        let store = seeded();
        let recent = |unread_only| AgentQuery::Recent {
            since: 0,
            limit: 50,
            unread_only,
        };
        let answer = run(&store, recent(false), &all()).unwrap();
        assert_eq!(
            ids(&answer, "messages", "conversation_id"),
            ["chat:unread", "chat:a", "chat:b"]
        );
        assert_eq!(answer["more"], false);
        let answer = run(&store, recent(true), &all()).unwrap();
        assert_eq!(ids(&answer, "messages", "conversation"), ["Standup"]);
    }

    #[test]
    fn read_unknown_chat() {
        let store = seeded();
        let read = |key: &str| AgentQuery::ReadMessages {
            conversation: key.into(),
            before: None,
            limit: 10,
        };
        assert_eq!(run(&store, read("chat:nope"), &all()), Err("not_found"));
        assert_eq!(run(&store, read("nonsense"), &all()), Err("bad_request"));
        // A known chat without cached messages is empty, not missing.
        let store = open(&[chat("empty", ChatKind::Group, Some("E"), 1, false)]);
        let answer = run(&store, read("chat:empty"), &all()).unwrap();
        assert!(answer["messages"].as_array().unwrap().is_empty());
        assert_eq!(answer["more"], false);
    }

    #[test]
    fn read_messages_pages_with_a_cursor() {
        let conversation = ConversationId::Chat("a".into());
        let mut store = open(&[chat("a", ChatKind::OneOnOne, None, 9, false)]);
        let batch: Vec<Message> = (1..=5)
            .map(|n| message(&conversation, &format!("m{n}"), n * 100, &format!("t{n}")))
            .collect();
        store.save_messages(&conversation, &batch).unwrap();
        let page = |before, limit| AgentQuery::ReadMessages {
            conversation: "chat:a".into(),
            before,
            limit,
        };
        let first = run(&store, page(None, 2), &all()).unwrap();
        assert_eq!(ids(&first, "messages", "message_id"), ["m4", "m5"]);
        assert_eq!(first["more"], true);
        assert_eq!(first["before"], 400);
        assert_eq!(first["conversation_id"], "chat:a");
        let second = run(&store, page(Some(400), 10), &all()).unwrap();
        assert_eq!(second["messages"].as_array().unwrap().len(), 3);
        assert_eq!(second["more"], false);
        assert!(second.get("before").is_none());
    }

    #[test]
    fn list_channels_names() {
        let mut store = Store::in_memory().unwrap();
        store
            .save_teams(&[Team {
                id: "t1".into(),
                name: "Platform".into(),
                ..Default::default()
            }])
            .unwrap();
        store
            .save_channels(
                "t1",
                &[Channel {
                    id: "19:c@thread.tacv2".into(),
                    team_id: "t1".into(),
                    name: "General".into(),
                    ..Default::default()
                }],
            )
            .unwrap();
        let id = ConversationId::Channel {
            team: "t1".into(),
            channel: "19:c@thread.tacv2".into(),
        };
        store
            .save_messages(&id, &[message(&id, "p1", 7_000, "post")])
            .unwrap();
        let answer = run(&store, AgentQuery::ListChannels, &all()).unwrap();
        let channels = answer["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["team"], "Platform");
        assert_eq!(channels[0]["channel"], "General");
        assert_eq!(channels[0]["id"], id.key());
        assert!(channels[0]["last_message_at"].is_string());
        // Channels follow their own list.
        let answer = run(&store, AgentQuery::ListChannels, &only(&[])).unwrap();
        assert!(answer["channels"].as_array().unwrap().is_empty());
    }

    #[test]
    fn answer_under_60000_bytes() {
        let conversation = ConversationId::Chat("a".into());
        let mut store = open(&[chat("a", ChatKind::OneOnOne, None, 9, false)]);
        let big = "word ".repeat(400);
        let batch: Vec<Message> = (1..=200)
            .map(|n| message(&conversation, &format!("m{n}"), n, &format!("lunch {big}")))
            .collect();
        store.save_messages(&conversation, &batch).unwrap();
        let read_all = AgentQuery::ReadMessages {
            conversation: "chat:a".into(),
            before: None,
            limit: 200,
        };
        let queries = [
            read_all.clone(),
            AgentQuery::Recent {
                since: 0,
                limit: 200,
                unread_only: false,
            },
            AgentQuery::Search {
                query: "lunch".into(),
                limit: 200,
            },
        ];
        for query in queries {
            let answer = run(&store, query.clone(), &all()).unwrap();
            let size = serde_json::to_string(&answer).unwrap().len();
            assert!(size < MAX_ANSWER, "{query:?} answered {size} bytes");
        }
        let answer = run(&store, read_all, &all()).unwrap();
        assert_eq!(answer["more"], true);
        assert!(answer["before"].is_number());
    }
}
