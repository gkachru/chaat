//! App actions: chats.

use super::super::*;

impl App {
    pub(in crate::app) fn apply_open_new_chat(&mut self) {
        if !self.ensure_scope(crate::auth::CHAT_CREATE_SCOPE) {
            return;
        }
        self.new_chat = NewChat::default();
        self.people = People::default();
        self.overlay = Some(Overlay::NewChat);
    }

    pub(in crate::app) fn apply_open_add_people(&mut self, chat: String) {
        self.add_people = AddPeople::default();
        self.people = People::default();
        self.overlay = Some(Overlay::AddPeople(chat));
    }

    pub(in crate::app) fn apply_add_members(
        &mut self,
        chat: String,
        members: Vec<Member>,
        history: bool,
    ) {
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
            self.backend.send(AccountCommand::AddMembers {
                chat,
                users,
                history,
            });
        }
    }

    pub(in crate::app) fn apply_leave_chat(&mut self, chat: String) {
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
            self.backend.send(AccountCommand::LeaveChat(chat));
        }
    }

    pub(in crate::app) fn apply_search_people(&mut self, query: String) {
        self.people.query = query.clone();
        self.people.results.clear();
        if query.trim().chars().count() < 2 {
            return;
        }
        if self.demo {
            self.people.results = crate::demo::search_people(self, &query);
        } else {
            self.backend.send(AccountCommand::SearchPeople(query));
        }
    }

    pub(in crate::app) fn create_chat(&mut self, members: Vec<Member>, topic: Option<String>) {
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
        self.backend.send(AccountCommand::CreateChat {
            others: members.into_iter().map(|m| m.user_id).collect(),
            topic,
        });
        self.toast(ToastKind::Info, "Starting the chat…");
    }
}
