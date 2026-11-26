//! App events: chats.

use super::super::*;

impl App {
    pub(in crate::app) fn handle_teams(&mut self, teams: Vec<Team>) {
        if self.expanded_teams.is_empty()
            && let Some(first) = teams.first()
        {
            let first = first.id.clone();
            self.expanded_teams.insert(first);
        }
        self.teams = teams;
    }

    pub(in crate::app) fn handle_channels(&mut self, team: Option<String>, channels: Vec<Channel>) {
        match team {
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
        }
    }

    pub(in crate::app) fn handle_presence(&mut self, list: Vec<(String, Presence)>) {
        let before =
            (!self.settings.notify.follow_available.is_empty()).then(|| self.presence.clone());
        self.presence.extend(list);
        if let Some(before) = before {
            self.observe_presence_for_follow(&before);
        }
    }

    pub(in crate::app) fn handle_my_presence(&mut self, presence: Presence) {
        let me = self.me().to_owned();
        if !me.is_empty() {
            self.presence.insert(me, presence.clone());
        }
        self.observe_call(presence.activity.as_deref());
        self.my_presence = Some(presence);
        self.sync_plugin_presence();
    }

    pub(in crate::app) fn handle_photo(
        &mut self,
        user: String,
        bytes: Option<Vec<u8>>,
        ctx: &egui::Context,
    ) {
        let state =
            match bytes.and_then(|bytes| register_image(ctx, &format!("photo-{user}"), bytes)) {
                Some(uri) => Image::Ready(uri),
                None => Image::Missing,
            };
        self.photos.insert(user, state);
    }

    pub(in crate::app) fn handle_media(
        &mut self,
        url: String,
        bytes: Option<Vec<u8>>,
        ctx: &egui::Context,
    ) {
        let key = format!("media-{:016x}", fnv(&url));
        let state = match bytes.and_then(|bytes| register_image(ctx, &key, bytes)) {
            Some(uri) => Image::Ready(uri),
            None => Image::Missing,
        };
        self.media.insert(url, state);
    }

    pub(in crate::app) fn handle_people(&mut self, query: String, people: Vec<Member>) {
        if !self.hidden && query == self.people.query {
            self.people.results = people;
        }
    }

    pub(in crate::app) fn handle_members(&mut self, chat: String, members: Vec<Member>) {
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

    pub(in crate::app) fn handle_members_added(
        &mut self,
        chat: String,
        added: Vec<String>,
        failed: Vec<String>,
    ) {
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

    pub(in crate::app) fn handle_left_chat(&mut self, chat: String) {
        self.leaving.remove(&chat);
        self.chats.retain(|c| c.id != chat);
        self.left.insert(chat);
    }

    pub(in crate::app) fn handle_leave_failed(&mut self, chat: String, admin: bool) {
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

    pub(in crate::app) fn handle_chat_created(&mut self, chat: Box<Chat>) {
        let chat = *chat;
        let id = chat.id.clone();
        self.chats.retain(|c| c.id != id);
        self.chats.insert(0, chat);
        if !self.hidden {
            self.open_conversation(ConversationId::Chat(id));
        }
    }

    pub(in crate::app) fn handle_loading(&mut self, resource: Resource, on: bool) {
        if on {
            self.loading.insert(resource);
        } else {
            self.loading.remove(&resource);
        }
    }

    pub(in crate::app) fn on_chats(&mut self, mut chats: Vec<Chat>, cached: bool) {
        // A chat being left stays out of the list until Graph answers.
        if !cached {
            // Once Graph stops listing a left chat, forget it.
            let listed: HashSet<&str> = chats.iter().map(|c| c.id.as_str()).collect();
            self.left.retain(|id| listed.contains(id.as_str()));
        }
        chats.retain(|c| !self.leaving.contains_key(&c.id) && !self.left.contains(&c.id));
        self.keep_local_chat_flags(&mut chats);
        chats.sort_by_key(Chat::list_order);
        let me = self.me().to_owned();
        let mut notices = Vec::new();
        if !cached && !self.chats.is_empty() {
            for chat in &chats {
                let Some(preview) = &chat.preview else {
                    continue;
                };
                let previous = self.chat(&chat.id).and_then(|c| c.preview.as_ref());
                let new = previous
                    .is_none_or(|p| p.message_id != preview.message_id && preview.at > p.at);
                if new && preview.author_id.as_deref() != Some(me.as_str()) && !preview.deleted {
                    notices.push((
                        chat.id.clone(),
                        NoticeItem {
                            message: preview.message_id.clone(),
                            author: preview.author.clone(),
                            text: preview.text.clone(),
                            mentions_me: false,
                            urgent: false,
                            created: preview.at,
                        },
                    ));
                }
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
        self.keep_newer_previews(&mut chats);
        self.chats = chats;
        // One toast per chat; the kind and title come from the new list.
        for (chat, item) in notices {
            self.notify_batch(&ConversationId::Chat(chat), vec![item]);
        }
    }

    /// A preview newer than the list's, taken from a message the chat has
    /// loaded, stays: Graph's `lastMessagePreview` can lag behind the
    /// messages (a meeting chat's "Meeting ended" is missing from it), and
    /// the chat would jump down the list and back each time it is opened.
    fn keep_newer_previews(&self, chats: &mut [Chat]) {
        let mut moved = false;
        for chat in chats.iter_mut() {
            let Some(local) = self
                .chats
                .iter()
                .find(|c| c.id == chat.id)
                .and_then(|c| c.preview.as_ref())
            else {
                continue;
            };
            let newer = chat.preview.as_ref().is_none_or(|p| local.at > p.at);
            let loaded = self
                .conversations
                .get(&ConversationId::Chat(chat.id.clone()))
                .is_some_and(|c| c.messages.iter().any(|m| m.id == local.message_id));
            if newer && loaded {
                chat.preview = Some(local.clone());
                moved = true;
            }
        }
        if moved {
            chats.sort_by_key(Chat::list_order);
        }
    }

    pub(in crate::app) fn mark_read(&mut self, chat: &str) {
        if self.hidden || self.keeps_unread(chat) {
            return;
        }
        let account = self.id.clone().unwrap_or_default();
        self.urgent.stop(
            &account,
            &ConversationId::Chat(chat.to_owned()),
            crate::time::now(),
        );
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
            self.backend.send(AccountCommand::MarkRead(chat.to_owned()));
        }
    }
}
