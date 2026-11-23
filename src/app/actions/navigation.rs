//! App actions: navigation.

use super::super::*;

impl App {
    pub(in crate::app) fn apply_navigate(&mut self, section: Section) {
        self.settings_page = None;
        if self.section != section {
            self.section = section;
            self.open = match (section, &self.open) {
                (Section::Chats, Some(open @ ConversationId::Chat(_))) => Some(open.clone()),
                (Section::Teams, Some(open @ ConversationId::Channel { .. })) => Some(open.clone()),
                _ => None,
            };
        }
        self.fetch_view(Trigger::Opened);
    }

    pub(in crate::app) fn apply_open_message(
        &mut self,
        conversation: ConversationId,
        message: String,
    ) {
        // Already open (the pinned bar): only jump. Reopening would pull the
        // view to the end and the cursor into the composer.
        if self.open.as_ref() != Some(&conversation) || self.settings_page.is_some() {
            self.open_conversation(conversation);
        }
        self.jump_to = Some(message);
        self.overlay = None;
    }

    pub(in crate::app) fn apply_toggle_team(&mut self, team: String) {
        if !self.expanded_teams.remove(&team) {
            self.expanded_teams.insert(team.clone());
            if self.channels.get(&team).is_none_or(Vec::is_empty) {
                self.fetch(Resource::Channels(team), Trigger::Opened);
            }
        }
    }

    pub(in crate::app) fn apply_open_url(&mut self, url: String) {
        if url.starts_with("https://") || url.starts_with("http://") {
            self.open_link(&url);
        } else if url.starts_with("mailto:") {
            open_external(&url);
        }
    }

    pub(in crate::app) fn open_conversation(&mut self, conversation: ConversationId) {
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
        if self.open.as_ref() != Some(&conversation) {
            self.find = None;
        }
        self.open = Some(conversation.clone());
        self.trim_conversations();
        if needs_cache && !self.demo {
            self.backend
                .send(AccountCommand::CachedMessages(conversation.clone()));
        }
        self.focus_composer = true;
        self.scroll_to_end = 4;
        self.fetch(Resource::Messages(conversation.clone()), Trigger::Opened);
        if let ConversationId::Chat(chat) = &conversation {
            self.open_pins(chat);
            // Opening the chat ends its urgent repeats.
            if !self.hidden {
                let account = self.id.clone().unwrap_or_default();
                self.urgent
                    .stop(&account, &conversation, crate::time::now());
            }
            // A chat marked unread tells Graph it is read again.
            self.mark_chat_read(chat.clone());
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
                self.backend.send(AccountCommand::Presence(members));
            }
        }
    }

    /// SQLite keeps the full history. RAM keeps the current conversation,
    /// pending/failed sends, and a small set of recently opened conversations.
    pub(in crate::app) fn trim_conversations(&mut self) {
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

    pub(in crate::app) fn step(&mut self, delta: i32) {
        let me = self.me().to_owned();
        let list: Vec<ConversationId> = match self.section {
            Section::Chats => self
                .chats
                .iter()
                .filter(|c| chat_filter::listed(self, c, &me))
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
}
