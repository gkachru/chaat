//! The local cache: one SQLite database per account.
//!
//! Chats, teams, channels, messages, meetings and photos are kept so Chaat
//! opens instantly and navigates without waiting for the network. Rows hold
//! the model types as JSON next to the columns queries need, so adding a
//! model field needs no migration. Message text is indexed with FTS5 for
//! instant local search.
//!
//! The cache holds the person's work messages: the file is created private
//! (0600 on Unix) in a private folder. Encrypting it with a key from the OS
//! credential store is on the roadmap (docs/roadmap.md).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Channel, Chat, ConversationId, Meeting, Message, Preview, SearchHit, Team};
use crate::outbox::{OutboxEntry, StoredDraft};

const SCHEMA_VERSION: i32 = 1;
/// The version of the HTML conversion cached messages went through. Raise it
/// when `html::parse` reads Teams messages differently; older cached
/// messages are then dropped and fetched again (2: tables, code blocks;
/// 3: nested and lettered lists).
const MESSAGES_FORMAT: i64 = 3;
const MEDIA_BUDGET: usize = 128 * 1024 * 1024;

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = crate::paths::create_private_dir(dir);
        }
        let db = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Self::setup(db)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(db: Connection) -> rusqlite::Result<Self> {
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            // A newer Chaat wrote this cache. It is only a cache: start over.
            log::warn!("cache schema {version} is newer than {SCHEMA_VERSION}; rebuilding it");
            db.execute_batch(
                "DROP TABLE IF EXISTS chats; DROP TABLE IF EXISTS teams; DROP TABLE IF EXISTS channels;
                 DROP TABLE IF EXISTS messages; DROP TABLE IF EXISTS messages_fts; DROP TABLE IF EXISTS meetings;
                 DROP TABLE IF EXISTS photos; DROP TABLE IF EXISTS media; DROP TABLE IF EXISTS meta;",
            )?;
            db.pragma_update(None, "user_version", 0)?;
        }
        let store = Self { db };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let version: i32 = self
            .db
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 1 {
            self.db.execute_batch(
                "BEGIN;
                CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS chats (
                    id TEXT PRIMARY KEY,
                    activity INTEGER NOT NULL,
                    json TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS teams (id TEXT PRIMARY KEY, json TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS channels (
                    team TEXT NOT NULL,
                    id TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    json TEXT NOT NULL,
                    PRIMARY KEY (team, id)
                );
                CREATE TABLE IF NOT EXISTS messages (
                    conversation TEXT NOT NULL,
                    id TEXT NOT NULL,
                    created INTEGER NOT NULL,
                    modified INTEGER NOT NULL,
                    json TEXT NOT NULL,
                    PRIMARY KEY (conversation, id)
                );
                CREATE INDEX IF NOT EXISTS messages_by_time ON messages (conversation, created);
                CREATE INDEX IF NOT EXISTS messages_by_created ON messages (created);
                CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                    body, author, conversation UNINDEXED, id UNINDEXED,
                    tokenize = 'unicode61 remove_diacritics 2'
                );
                CREATE TABLE IF NOT EXISTS meetings (id TEXT PRIMARY KEY, start INTEGER NOT NULL, json TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS photos (user TEXT PRIMARY KEY, fetched INTEGER NOT NULL, bytes BLOB);
                PRAGMA user_version = 1;
                COMMIT;",
            )?;
        }
        // This optional cache is additive: older v1 clients can still read
        // the database without rebuilding the person's message history.
        self.db.execute_batch(
            "CREATE TABLE IF NOT EXISTS media (
                url TEXT PRIMARY KEY, bytes BLOB NOT NULL, last_used INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS media_by_use ON media(last_used);
             CREATE TABLE IF NOT EXISTS drafts (conversation TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS outbox (
                local_id TEXT PRIMARY KEY, created INTEGER NOT NULL, json TEXT NOT NULL
             );",
        )?;
        self.renew_messages_format()
    }

    /// Messages are cached already converted from Teams' HTML. When the
    /// conversion learns something (tables, say), older rows would keep the
    /// old reading for good, so they go and are fetched again when their
    /// chat opens. Chats, drafts and the outbox stay.
    fn renew_messages_format(&self) -> rusqlite::Result<()> {
        let stored: Option<i64> = self
            .db
            .query_row(
                "SELECT value FROM meta WHERE key = 'messages_format'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| value.parse().ok());
        if stored.is_some_and(|stored| stored >= MESSAGES_FORMAT) {
            return Ok(());
        }
        self.db.execute_batch(&format!(
            "BEGIN;
             DELETE FROM messages;
             DELETE FROM messages_fts;
             INSERT OR REPLACE INTO meta (key, value) VALUES ('messages_format', '{MESSAGES_FORMAT}');
             COMMIT;"
        ))
    }

    /// Encoded Graph-hosted content, so RAM eviction does not force a download.
    pub fn media(&self, url: &str) -> rusqlite::Result<Option<Vec<u8>>> {
        let bytes = self
            .db
            .query_row("SELECT bytes FROM media WHERE url = ?1", [url], |r| {
                r.get(0)
            })
            .optional()?;
        if bytes.is_some() {
            self.db.execute("UPDATE media SET last_used = (SELECT COALESCE(MAX(last_used), 0) + 1 FROM media) WHERE url = ?1", [url])?;
        }
        Ok(bytes)
    }

    pub fn save_media(&mut self, url: &str, bytes: &[u8]) -> rusqlite::Result<()> {
        self.save_media_with_budget(url, bytes, MEDIA_BUDGET)
    }

    fn save_media_with_budget(
        &mut self,
        url: &str,
        bytes: &[u8],
        budget: usize,
    ) -> rusqlite::Result<()> {
        if bytes.len() > budget {
            return Ok(());
        }
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO media (url, bytes, last_used)
            VALUES (?1, ?2, (SELECT COALESCE(MAX(last_used), 0) + 1 FROM media))
            ON CONFLICT(url) DO UPDATE SET bytes = excluded.bytes, last_used = excluded.last_used",
            params![url, bytes],
        )?;
        let mut total: i64 = tx.query_row(
            "SELECT COALESCE(SUM(length(bytes)), 0) FROM media",
            [],
            |r| r.get(0),
        )?;
        while total > budget as i64 {
            let (oldest, size): (String, i64) = tx.query_row(
                "SELECT url, length(bytes) FROM media ORDER BY last_used LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            tx.execute("DELETE FROM media WHERE url = ?1", [oldest])?;
            total -= size;
        }
        tx.commit()
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.db
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Replaces the chat list with Graph's answer. Chats Graph no longer
    /// lists keep their messages until the cache is cleared.
    pub fn save_chats(&mut self, chats: &[Chat]) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM chats", [])?;
        {
            let mut insert =
                tx.prepare("INSERT INTO chats (id, activity, json) VALUES (?1, ?2, ?3)")?;
            for chat in chats {
                insert.execute(params![chat.id, chat.activity(), to_json(chat)])?;
            }
        }
        tx.commit()
    }

    /// Gives each chat the preview of its newest cached message when that is
    /// newer than the one it has. Graph's `lastMessagePreview` can lag
    /// behind the messages (a meeting chat's "Meeting ended" is missing from
    /// it), and the chat would otherwise jump up the list only once opened.
    pub fn freshen_previews(&self, chats: &mut [Chat]) {
        for chat in chats.iter_mut() {
            let conversation = ConversationId::Chat(chat.id.clone());
            let Ok(recent) = self.messages(&conversation, 8) else {
                continue;
            };
            if let Some(last) = recent.iter().rev().find(|m| m.membership.is_none())
                && Preview::superseded_by(chat.preview.as_ref(), last)
            {
                chat.preview = Some(Preview::of(last));
            }
        }
    }

    pub fn upsert_chat(&self, chat: &Chat) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO chats (id, activity, json) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET activity = excluded.activity, json = excluded.json",
            params![chat.id, chat.activity(), to_json(chat)],
        )?;
        Ok(())
    }

    pub fn chats(&self) -> rusqlite::Result<Vec<Chat>> {
        let mut query = self
            .db
            .prepare("SELECT json FROM chats ORDER BY activity DESC")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| from_json(&json.ok()?)).collect())
    }

    pub fn save_teams(&mut self, teams: &[Team]) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM teams", [])?;
        {
            let mut insert = tx.prepare("INSERT INTO teams (id, json) VALUES (?1, ?2)")?;
            for team in teams {
                insert.execute(params![team.id, to_json(team)])?;
            }
        }
        tx.commit()
    }

    pub fn teams(&self) -> rusqlite::Result<Vec<Team>> {
        let mut query = self.db.prepare("SELECT json FROM teams")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        let mut teams: Vec<Team> = rows.filter_map(|json| from_json(&json.ok()?)).collect();
        teams.sort_by_key(|team| team.name.to_lowercase());
        Ok(teams)
    }

    pub fn save_channels(&mut self, team: &str, channels: &[Channel]) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM channels WHERE team = ?1", [team])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO channels (team, id, position, json) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (position, channel) in channels.iter().enumerate() {
                insert.execute(params![team, channel.id, position as i64, to_json(channel)])?;
            }
        }
        tx.commit()
    }

    pub fn channels(&self) -> rusqlite::Result<Vec<Channel>> {
        let mut query = self
            .db
            .prepare("SELECT json FROM channels ORDER BY team, position")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| from_json(&json.ok()?)).collect())
    }

    /// Inserts or replaces messages and their search entries.
    pub fn save_messages(
        &mut self,
        conversation: &ConversationId,
        messages: &[Message],
    ) -> rusqlite::Result<()> {
        let key = conversation.key();
        let tx = self.db.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO messages (conversation, id, created, modified, json) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(conversation, id) DO UPDATE SET created = excluded.created,
                 modified = excluded.modified, json = excluded.json",
            )?;
            let mut forget =
                tx.prepare("DELETE FROM messages_fts WHERE conversation = ?1 AND id = ?2")?;
            let mut index = tx.prepare(
                "INSERT INTO messages_fts (body, author, conversation, id) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for message in messages {
                insert.execute(params![
                    key,
                    message.id,
                    message.created,
                    message.modified,
                    to_json(message)
                ])?;
                forget.execute(params![key, message.id])?;
                if !message.system && !message.deleted {
                    let mut text = message.body.to_plain();
                    for reply in &message.replies {
                        text.push('\n');
                        text.push_str(&reply.body.to_plain());
                    }
                    if let Some(subject) = &message.subject {
                        text = format!("{subject}\n{text}");
                    }
                    index.execute(params![text, message.author, key, message.id])?;
                }
            }
        }
        tx.commit()
    }

    /// The newest `limit` messages of a conversation, oldest first.
    pub fn messages(
        &self,
        conversation: &ConversationId,
        limit: usize,
    ) -> rusqlite::Result<Vec<Message>> {
        let mut query = self.db.prepare(
            "SELECT json FROM (SELECT json, created FROM messages WHERE conversation = ?1
             ORDER BY created DESC LIMIT ?2) ORDER BY created ASC",
        )?;
        let rows = query.query_map(params![conversation.key(), limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        Ok(rows.filter_map(|json| from_json(&json.ok()?)).collect())
    }

    /// The newest `limit` cached messages of every conversation created after
    /// `since`, newest first. Reads the cache only. Uses the `created` index.
    pub fn recent_messages(&self, since: i64, limit: usize) -> rusqlite::Result<Vec<Message>> {
        let mut query = self.db.prepare(
            "SELECT conversation, json FROM messages WHERE created > ?1
             ORDER BY created DESC LIMIT ?2",
        )?;
        let rows = query.query_map(params![since, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows
            .filter_map(|row| {
                let (key, json) = row.ok()?;
                let mut message: Message = from_json(&json)?;
                if message.conversation.is_none() {
                    message.conversation = ConversationId::from_key(&key);
                }
                Some(message)
            })
            .collect())
    }

    /// Cached history immediately before the oldest visible message.
    pub fn messages_before(
        &self,
        conversation: &ConversationId,
        before: i64,
        limit: usize,
    ) -> rusqlite::Result<Vec<Message>> {
        let mut query = self.db.prepare(
            "SELECT json FROM (SELECT json, created FROM messages WHERE conversation = ?1 AND created < ?2
             ORDER BY created DESC LIMIT ?3) ORDER BY created ASC",
        )?;
        let rows = query.query_map(params![conversation.key(), before, limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        Ok(rows.filter_map(|json| from_json(&json.ok()?)).collect())
    }

    /// The newest change Chaat has seen in a conversation, for date-ranged
    /// refreshes.
    pub fn last_modified(&self, conversation: &ConversationId) -> Option<i64> {
        self.db
            .query_row(
                "SELECT MAX(modified) FROM messages WHERE conversation = ?1",
                [conversation.key()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
    }

    /// When the oldest cached message of a conversation was written.
    pub fn oldest(&self, conversation: &ConversationId) -> Option<i64> {
        self.db
            .query_row(
                "SELECT MIN(created) FROM messages WHERE conversation = ?1",
                [conversation.key()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
    }

    pub fn delete_message(&self, conversation: &ConversationId, id: &str) -> rusqlite::Result<()> {
        let key = conversation.key();
        self.db.execute(
            "DELETE FROM messages WHERE conversation = ?1 AND id = ?2",
            params![key, id],
        )?;
        self.db.execute(
            "DELETE FROM messages_fts WHERE conversation = ?1 AND id = ?2",
            params![key, id],
        )?;
        Ok(())
    }

    /// Local full-text search, newest first. Each word must match a prefix.
    pub fn search(&self, query: &str, limit: usize) -> rusqlite::Result<Vec<SearchHit>> {
        let Some(expression) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut statement = self.db.prepare(
            "SELECT f.conversation, f.id, f.author, snippet(messages_fts, 0, '', '', '…', 12), m.created
             FROM messages_fts f JOIN messages m ON m.conversation = f.conversation AND m.id = f.id
             WHERE messages_fts MATCH ?1 ORDER BY m.created DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![expression, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        Ok(rows
            .filter_map(Result::ok)
            .filter_map(|(conversation, id, author, snippet, at)| {
                Some(SearchHit {
                    conversation: ConversationId::from_key(&conversation)?,
                    message_id: id,
                    author,
                    snippet: snippet.replace('\n', " "),
                    at,
                })
            })
            .collect())
    }

    /// Local search inside one conversation, newest first.
    pub fn search_in(
        &self,
        conversation: &ConversationId,
        query: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<SearchHit>> {
        let Some(expression) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut statement = self.db.prepare(
            "SELECT f.conversation, f.id, f.author, snippet(messages_fts, 0, '', '', '…', 12), m.created
             FROM messages_fts f JOIN messages m ON m.conversation = f.conversation AND m.id = f.id
             WHERE messages_fts MATCH ?1 AND f.conversation = ?2 ORDER BY m.created DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![expression, conversation.key(), limit as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        Ok(rows
            .filter_map(Result::ok)
            .filter_map(|(conversation, id, author, snippet, at)| {
                Some(SearchHit {
                    conversation: ConversationId::from_key(&conversation)?,
                    message_id: id,
                    author,
                    snippet: snippet.replace('\n', " "),
                    at,
                })
            })
            .collect())
    }

    /// Saves a conversation's draft; `None` removes it.
    pub fn save_draft(
        &self,
        conversation: &ConversationId,
        draft: Option<&StoredDraft>,
    ) -> rusqlite::Result<()> {
        match draft {
            Some(draft) => self.db.execute(
                "INSERT INTO drafts (conversation, json) VALUES (?1, ?2)
                 ON CONFLICT(conversation) DO UPDATE SET json = excluded.json",
                params![conversation.key(), to_json(draft)],
            )?,
            None => self.db.execute(
                "DELETE FROM drafts WHERE conversation = ?1",
                [conversation.key()],
            )?,
        };
        Ok(())
    }

    pub fn drafts(&self) -> rusqlite::Result<Vec<(ConversationId, StoredDraft)>> {
        let mut query = self.db.prepare("SELECT conversation, json FROM drafts")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows
            .filter_map(Result::ok)
            .filter_map(|(key, json)| Some((ConversationId::from_key(&key)?, from_json(&json)?)))
            .collect())
    }

    pub fn save_outbox(&self, entry: &OutboxEntry) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO outbox (local_id, created, json) VALUES (?1, ?2, ?3)
             ON CONFLICT(local_id) DO UPDATE SET created = excluded.created, json = excluded.json",
            params![entry.payload.local_id, entry.created, to_json(entry)],
        )?;
        Ok(())
    }

    pub fn delete_outbox(&self, local_id: &str) -> rusqlite::Result<()> {
        self.db
            .execute("DELETE FROM outbox WHERE local_id = ?1", [local_id])?;
        Ok(())
    }

    /// Unconfirmed messages, oldest first.
    pub fn outbox(&self) -> rusqlite::Result<Vec<OutboxEntry>> {
        let mut query = self
            .db
            .prepare("SELECT json FROM outbox ORDER BY created ASC, rowid ASC")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| from_json(&json.ok()?)).collect())
    }

    pub fn save_meetings(&mut self, meetings: &[Meeting]) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM meetings", [])?;
        {
            let mut insert =
                tx.prepare("INSERT INTO meetings (id, start, json) VALUES (?1, ?2, ?3)")?;
            for meeting in meetings {
                insert.execute(params![meeting.id, meeting.start, to_json(meeting)])?;
            }
        }
        tx.commit()
    }

    pub fn meetings(&self) -> rusqlite::Result<Vec<Meeting>> {
        let mut query = self
            .db
            .prepare("SELECT json FROM meetings ORDER BY start")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| from_json(&json.ok()?)).collect())
    }

    /// A cached photo: `Some(None)` means the person has none.
    pub fn photo(&self, user: &str, max_age_ms: i64) -> Option<Option<Vec<u8>>> {
        self.db
            .query_row(
                "SELECT bytes FROM photos WHERE user = ?1 AND fetched > ?2",
                params![user, crate::time::now() - max_age_ms],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn save_photo(&self, user: &str, bytes: Option<&[u8]>) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO photos (user, fetched, bytes) VALUES (?1, ?2, ?3)
             ON CONFLICT(user) DO UPDATE SET fetched = excluded.fetched, bytes = excluded.bytes",
            params![user, crate::time::now(), bytes],
        )?;
        Ok(())
    }

    /// Counts for the performance overlay and diagnostics (no content).
    pub fn stats(&self) -> (i64, i64) {
        let count = |table: &str| {
            self.db
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap_or_default()
        };
        (count("chats"), count("messages"))
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn from_json<T: serde::de::DeserializeOwned>(json: &str) -> Option<T> {
    serde_json::from_str(json).ok()
}

/// Turns typed words into an FTS5 expression: each word quoted (so
/// operators in the text are literal) and matched as a prefix.
fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| word.replace('"', ""))
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{word}\"*"))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::RichText;
    use crate::model::Preview;

    fn message(id: &str, created: i64, text: &str) -> Message {
        Message {
            id: id.into(),
            author: "Ada".into(),
            author_id: Some("u-ada".into()),
            created,
            modified: created,
            body: RichText::plain(text),
            ..Default::default()
        }
    }

    /// A meeting chat's "Meeting ended" is newer than Graph's preview: the
    /// cached message wins, at startup as after a refresh.
    #[test]
    fn previews_follow_newer_cached_messages() {
        let mut store = Store::in_memory().unwrap();
        let chat = |id: &str, preview: &str, at: i64| Chat {
            id: id.into(),
            preview: Some(Preview {
                message_id: preview.into(),
                at,
                ..Default::default()
            }),
            ..Default::default()
        };
        let meeting = ConversationId::Chat("19:meeting".into());
        let ended = Message {
            system: true,
            ..message("ended", 300, "")
        };
        let added = Message {
            membership: Some(crate::model::Membership {
                change: crate::model::MemberChange::Added,
                by: None,
                members: Vec::new(),
            }),
            ..message("added", 400, "")
        };
        store
            .save_messages(&meeting, &[message("m1", 100, "hi"), ended, added])
            .unwrap();
        let mut chats = vec![chat("19:meeting", "m1", 100), chat("19:other", "o9", 900)];
        store.freshen_previews(&mut chats);
        let preview = chats[0].preview.as_ref().unwrap();
        assert_eq!((preview.message_id.as_str(), preview.at), ("ended", 300));
        assert!(preview.system);
        assert_eq!(chats[1].preview.as_ref().unwrap().message_id, "o9");

        // A newer preview from Graph stays.
        let mut chats = vec![chat("19:meeting", "m5", 500)];
        store.freshen_previews(&mut chats);
        assert_eq!(chats[0].preview.as_ref().unwrap().message_id, "m5");
    }

    #[test]
    fn chats_come_back_newest_first() {
        let mut store = Store::in_memory().unwrap();
        let chat = |id: &str, at: i64| Chat {
            id: id.into(),
            preview: Some(Preview {
                at,
                ..Default::default()
            }),
            ..Default::default()
        };
        store
            .save_chats(&[chat("old", 1), chat("new", 5), chat("mid", 3)])
            .unwrap();
        let ids: Vec<String> = store.chats().unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["new", "mid", "old"]);
    }

    #[test]
    fn recent_messages_span_conversations_newest_first() {
        let mut store = Store::in_memory().unwrap();
        let a = ConversationId::Chat("19:a".into());
        let b = ConversationId::Channel {
            team: "t".into(),
            channel: "19:c".into(),
        };
        store
            .save_messages(&a, &[message("old", 100, "x"), message("a2", 300, "x")])
            .unwrap();
        store.save_messages(&b, &[message("b1", 200, "x")]).unwrap();
        let ids = |since, limit| -> Vec<String> {
            store
                .recent_messages(since, limit)
                .unwrap()
                .into_iter()
                .map(|m| m.id)
                .collect()
        };
        assert_eq!(ids(150, 10), ["a2", "b1"]);
        assert_eq!(ids(0, 2), ["a2", "b1"]);
        assert_eq!(ids(300, 10), Vec::<String>::new());
        let found = store.recent_messages(150, 10).unwrap();
        assert_eq!(found[1].conversation, Some(b));
    }

    #[test]
    fn messages_page_and_update_in_place() {
        let mut store = Store::in_memory().unwrap();
        let conversation = ConversationId::Chat("19:a".into());
        let messages: Vec<Message> = (0..10)
            .map(|i| message(&i.to_string(), i, "hello"))
            .collect();
        store.save_messages(&conversation, &messages).unwrap();
        let newest = store.messages(&conversation, 3).unwrap();
        let earlier = store
            .messages_before(&conversation, newest[0].created, 3)
            .unwrap();
        assert_eq!(
            earlier.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["4", "5", "6"]
        );
        assert_eq!(
            newest.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["7", "8", "9"]
        );
        let mut edited = message("9", 9, "edited text");
        edited.modified = 100;
        store.save_messages(&conversation, &[edited]).unwrap();
        assert_eq!(
            store.messages(&conversation, 1).unwrap()[0].body.to_plain(),
            "edited text"
        );
        assert_eq!(store.last_modified(&conversation), Some(100));
        assert_eq!(store.stats(), (0, 10));
    }

    #[test]
    fn search_finds_prefixes_and_ignores_operators() {
        let mut store = Store::in_memory().unwrap();
        let chat = ConversationId::Chat("19:a".into());
        let channel = ConversationId::Channel {
            team: "t".into(),
            channel: "19:c".into(),
        };
        store
            .save_messages(
                &chat,
                &[
                    message("1", 1, "The launch is on Friday"),
                    message("2", 2, "Lunch?"),
                ],
            )
            .unwrap();
        store
            .save_messages(&channel, &[message("3", 3, "Launch checklist ready")])
            .unwrap();
        let hits = store.search("laun", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].conversation, channel);
        assert!(store.search("\"unbalanced AND OR (", 10).is_ok());
        assert!(store.search("   ", 10).unwrap().is_empty());
        // Diacritics fold.
        store
            .save_messages(&chat, &[message("4", 4, "Café opening")])
            .unwrap();
        assert_eq!(store.search("cafe", 10).unwrap().len(), 1);
        // Updated text replaces its index entry.
        store
            .save_messages(&chat, &[message("4", 4, "Tea opening")])
            .unwrap();
        assert!(store.search("cafe", 10).unwrap().is_empty());
    }

    #[test]
    fn files_reopen_with_their_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts").join("a").join("chaat.db");
        {
            let mut store = Store::open(&path).unwrap();
            store
                .save_teams(&[Team {
                    id: "t".into(),
                    name: "Design".into(),
                    ..Default::default()
                }])
                .unwrap();
            store.set_meta("k", "v").unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.teams().unwrap()[0].name, "Design");
        assert_eq!(store.meta("k").as_deref(), Some("v"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn photos_remember_absence() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.photo("u", 1000), None);
        store.save_photo("u", None).unwrap();
        assert_eq!(store.photo("u", 60_000), Some(None));
        store.save_photo("v", Some(b"jpeg")).unwrap();
        assert_eq!(store.photo("v", 60_000), Some(Some(b"jpeg".to_vec())));
    }

    #[test]
    fn media_cache_evicts_least_recently_used_bytes_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chaat.db");
        {
            let mut store = Store::open(&path).unwrap();
            store.save_media_with_budget("a", b"aaaa", 8).unwrap();
            store.save_media_with_budget("b", b"bbbb", 8).unwrap();
            assert_eq!(store.media("a").unwrap(), Some(b"aaaa".to_vec()));
            store.save_media_with_budget("c", b"cccc", 8).unwrap();
            assert_eq!(store.media("b").unwrap(), None);
            store
                .save_media_with_budget("too large", b"123456789", 8)
                .unwrap();
            assert_eq!(store.media("too large").unwrap(), None);
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.media("a").unwrap(), Some(b"aaaa".to_vec()));
        assert_eq!(store.media("c").unwrap(), Some(b"cccc".to_vec()));
    }

    #[test]
    fn drafts_round_trip_and_none_deletes() {
        let store = Store::in_memory().unwrap();
        let chat = ConversationId::Chat("19:a".into());
        let draft = StoredDraft {
            text: "hi".into(),
            reply_to: Some("m1".into()),
            mentions: vec![crate::model::MentionRef {
                user_id: "u".into(),
                name: "U".into(),
            }],
        };
        store.save_draft(&chat, Some(&draft)).unwrap();
        store.save_draft(&chat, Some(&draft)).unwrap();
        assert_eq!(store.drafts().unwrap(), vec![(chat.clone(), draft)]);
        store.save_draft(&chat, None).unwrap();
        assert!(store.drafts().unwrap().is_empty());
    }

    fn entry(local_id: &str, created: i64) -> OutboxEntry {
        OutboxEntry {
            payload: crate::outbox::SendPayload {
                local_id: local_id.into(),
                conversation: ConversationId::Chat("19:a".into()),
                html: "<p>hi</p>".into(),
                reply_to: None,
                mentions: vec![],
                files: vec![],
                images: vec![],
                recipients: vec![],
            },
            created,
            text: "hi".into(),
        }
    }

    #[test]
    fn outbox_newest_last_and_delete() {
        let store = Store::in_memory().unwrap();
        store.save_outbox(&entry("b", 20)).unwrap();
        store.save_outbox(&entry("a", 10)).unwrap();
        let ids: Vec<_> = store
            .outbox()
            .unwrap()
            .into_iter()
            .map(|e| e.payload.local_id)
            .collect();
        assert_eq!(ids, ["a", "b"]);
        store.delete_outbox("a").unwrap();
        assert_eq!(store.outbox().unwrap().len(), 1);
    }

    #[test]
    fn images_round_trip() {
        let store = Store::in_memory().unwrap();
        let image = crate::clipboard::encode_rgba(2, 2, &[255; 16]).unwrap();
        let mut e = entry("a", 10);
        e.payload.images = vec![image.clone()];
        e.payload.files = vec!["C:/x/y.txt".into()];
        store.save_outbox(&e).unwrap();
        let back = store.outbox().unwrap().remove(0);
        assert_eq!(back.payload.images, vec![image]);
        assert_eq!(back, e);
    }

    #[test]
    fn an_older_message_format_drops_cached_messages_but_keeps_chats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old-format.db");
        let chat = ConversationId::Chat("19:a".into());
        {
            let mut store = Store::open(&path).unwrap();
            store
                .save_chats(&[Chat {
                    id: "19:a".into(),
                    ..Default::default()
                }])
                .unwrap();
            store
                .save_messages(&chat, &[message("1", 1, "hello")])
                .unwrap();
            // As an older Chaat left it: no format recorded.
            store
                .db
                .execute("DELETE FROM meta WHERE key = 'messages_format'", [])
                .unwrap();
        }
        let mut store = Store::open(&path).unwrap();
        assert!(store.messages(&chat, 10).unwrap().is_empty());
        assert!(store.search("hello", 10).unwrap().is_empty());
        assert_eq!(store.chats().unwrap().len(), 1);
        // Messages cached in the current format stay across a reopen.
        store
            .save_messages(&chat, &[message("2", 2, "again")])
            .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.messages(&chat, 10).unwrap().len(), 1);
    }

    #[test]
    fn migration_keeps_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let chat = ConversationId::Chat("19:a".into());
        {
            let mut store = Store::open(&path).unwrap();
            store
                .save_messages(&chat, &[message("1", 1, "hello")])
                .unwrap();
            store
                .db
                .execute_batch("DROP TABLE drafts; DROP TABLE outbox;")
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.messages(&chat, 10).unwrap().len(), 1);
        store
            .save_draft(&chat, Some(&StoredDraft::default()))
            .unwrap();
        assert!(store.outbox().unwrap().is_empty());
    }

    #[test]
    fn search_in_scoped() {
        let mut store = Store::in_memory().unwrap();
        let a = ConversationId::Chat("19:a".into());
        let b = ConversationId::Chat("19:b".into());
        store
            .save_messages(&a, &[message("1", 1, "lunch at noon")])
            .unwrap();
        store
            .save_messages(&b, &[message("2", 2, "lunch later")])
            .unwrap();
        let hits = store.search_in(&a, "lunch", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].conversation, a);
        assert!(store.search_in(&a, "", 10).unwrap().is_empty());
    }
}
