//! App actions: messages.

use super::super::*;

impl App {
    pub(in crate::app) fn apply_edit(
        &mut self,
        conversation: ConversationId,
        message: String,
        text: String,
    ) {
        let html = html::compose(&text);
        if let Some(entry) = self.conversations.get_mut(&conversation)
            && let Some(found) = entry.find_mut(&message)
        {
            found.body = html::parse(&html);
            found.edited = true;
        }
        self.send_account(AccountCommand::Edit {
            conversation: conversation.clone(),
            message,
            html,
        });
        self.drafts.remove(&conversation);
    }

    pub(in crate::app) fn apply_delete(&mut self, conversation: ConversationId, message: String) {
        if let Some(entry) = self.conversations.get_mut(&conversation)
            && let Some(found) = entry.find_mut(&message)
        {
            found.deleted = true;
            found.body = html::RichText::default();
        }
        self.send_account(AccountCommand::Delete {
            conversation,
            message,
        });
    }

    pub(in crate::app) fn apply_react(
        &mut self,
        conversation: ConversationId,
        message: String,
        emoji: String,
        on: bool,
    ) {
        let me = self.me().to_owned();
        if let Some(entry) = self.conversations.get_mut(&conversation)
            && let Some(found) = entry.find_mut(&message)
        {
            toggle_reaction(&mut found.reactions, &emoji, &me, on);
        }
        self.send_account(AccountCommand::React {
            conversation,
            message,
            emoji,
            on,
        });
    }

    pub(in crate::app) fn apply_load_older(&mut self, conversation: ConversationId) {
        if !self.demo {
            let resource = Resource::Messages(conversation.clone());
            if self.loading.contains(&resource) {
                return;
            }
            let before = self.conversations.get(&conversation).and_then(|c| {
                c.messages
                    .iter()
                    .filter(|m| m.delivery == Delivery::Sent)
                    .map(|m| m.created)
                    .min()
            });
            self.loading.insert(resource);
            self.backend.send(AccountCommand::LoadOlder {
                conversation,
                before,
            });
        } else if let Some(entry) = self.conversations.get_mut(&conversation) {
            entry.more = false;
        }
    }

    pub(in crate::app) fn apply_load_replies(
        &mut self,
        conversation: ConversationId,
        message: String,
    ) {
        self.send_account(AccountCommand::LoadReplies {
            conversation,
            root: message,
        });
    }

    pub(in crate::app) fn apply_toggle_thread(&mut self, root: String) {
        if let Some(open) = self.open.clone() {
            let entry = self.conversations.entry(open).or_default();
            entry.heights.remove(&root);
            if !entry.expanded.remove(&root) {
                entry.expanded.insert(root);
            }
            entry.layout.invalidate();
        }
    }

    pub(in crate::app) fn apply_retry_send(
        &mut self,
        conversation: ConversationId,
        local_id: String,
    ) {
        if let Some(pending) = self.pending_sends.get(&local_id).cloned() {
            if let Some(entry) = self.conversations.get_mut(&conversation)
                && let Some(found) = entry.find_mut(&local_id)
            {
                found.delivery = Delivery::Sending;
            }
            self.backend
                .send(AccountCommand::Send(crate::outbox::SendPayload {
                    conversation,
                    local_id,
                    html: pending.html,
                    reply_to: pending.reply_to,
                    mentions: pending.mentions,
                    files: pending.files,
                    images: pending.images,
                    recipients: pending.recipients,
                }));
        }
    }

    pub(in crate::app) fn apply_find_next(&mut self, step: i32, ctx: &egui::Context) {
        self.find_remote_now(ctx);
        if let Some(find) = &mut self.find
            && !find.hits.is_empty()
        {
            let count = find.hits.len() as i64;
            find.current = (find.current as i64 + i64::from(step)).rem_euclid(count) as usize;
            self.jump_to = Some(find.hits[find.current].message_id.clone());
        }
    }

    pub(in crate::app) fn apply_delete_failed(
        &mut self,
        conversation: ConversationId,
        local_id: String,
    ) {
        if let Some(entry) = self.conversations.get_mut(&conversation) {
            entry.layout.invalidate();
            entry.heights.remove(&local_id);
            entry.messages.retain(|m| m.id != local_id);
            for message in &mut entry.messages {
                message.replies.retain(|r| r.id != local_id);
            }
        }
        self.pending_sends.remove(&local_id);
        self.send_account(AccountCommand::DropOutbox(local_id));
    }

    /// Asks for each photo not loaded yet, or whose texture was evicted;
    /// the demo has no directory to ask.
    pub(in crate::app) fn apply_load_photos(&mut self, users: Vec<String>, ctx: &egui::Context) {
        if self.demo {
            return;
        }
        for user in users {
            let missing = match self.photos.get(&user) {
                Some(Image::Ready(uri)) => !crate::images::contains(ctx, uri),
                None => true,
                _ => false,
            };
            if missing {
                self.photos.insert(user.clone(), Image::Loading);
                self.backend.send(AccountCommand::Photo(user));
            }
        }
    }

    pub(in crate::app) fn apply_load_media(&mut self, url: String, ctx: &egui::Context) {
        let missing = match self.media.get(&url) {
            Some(Image::Ready(uri)) => !crate::images::contains(ctx, uri),
            None => true,
            _ => false,
        };
        if missing {
            let demo = self.demo;
            self.media.insert(url.clone(), Image::Loading);
            if demo {
                self.media.insert(url, Image::Missing);
            } else {
                self.backend.send(AccountCommand::Media(url));
            }
        }
    }

    pub(in crate::app) fn apply_preview_image(&mut self, uri: String, ctx: &egui::Context) {
        if !crate::images::contains(ctx, &uri) {
            let local = self
                .accounts
                .iter()
                .flat_map(|a| {
                    a.drafts
                        .values()
                        .flat_map(|d| &d.images)
                        .chain(a.pending_sends.values().flat_map(|s| &s.images))
                })
                .find(|image| image.uri() == uri)
                .cloned();
            if let Some(image) = local {
                crate::images::include(ctx, uri.clone(), image.png);
            } else if let Some(url) = self.media.iter().find_map(|(url, state)| {
                matches!(state, Image::Ready(stored) if *stored == uri).then(|| url.clone())
            }) {
                self.apply(Action::LoadMedia(url), ctx);
            }
        }
        self.overlay = Some(Overlay::ImagePreview(uri));
    }

    pub(in crate::app) fn apply_search_messages(&mut self, query: String) {
        self.switcher.searched = query.clone();
        self.switcher.hits.clear();
        self.switcher.remote_hits.clear();
        if query.trim().is_empty() {
            return;
        }
        if self.demo {
            self.switcher.hits = self.local_search(&query);
        } else {
            self.backend.send(AccountCommand::Search(query));
        }
    }

    /// Opens or updates find. A query of two characters or more searches the
    /// local cache at once; Microsoft Search (three or more) waits until the
    /// query has been still for `FIND_REMOTE_DELAY` or Enter is pressed.
    pub(in crate::app) fn find_query(&mut self, query: String) {
        let Some(conversation) = self.open.clone() else {
            return;
        };
        let find = self.find.get_or_insert_with(Default::default);
        find.query = query.clone();
        if find.searched == query {
            return;
        }
        find.searched = query.clone();
        find.hits.clear();
        find.current = 0;
        self.find_remote = None;
        if query.trim().chars().count() >= 2 && !self.demo {
            if query.trim().chars().count() >= 3 {
                self.find_remote = Some((
                    conversation.clone(),
                    query.clone(),
                    Instant::now() + Self::FIND_REMOTE_DELAY,
                ));
            }
            self.backend.send(AccountCommand::SearchIn {
                conversation,
                query,
                remote: false,
            });
        }
    }

    /// Sends the waiting Microsoft Search once it is due (`now` at or past
    /// its deadline). Drops it when find closed, the
    /// conversation changed or the query moved on. While it waits, asks for
    /// one repaint at its deadline, so the frame that sends it runs.
    pub(in crate::app) fn send_due_find_search(&mut self, now: Instant, ctx: &egui::Context) {
        let Some((conversation, query, due)) = self.find_remote.take() else {
            return;
        };
        let current = self.open.as_ref() == Some(&conversation)
            && self
                .find
                .as_ref()
                .is_some_and(|find| find.searched == query);
        if !current {
            return;
        }
        if now < due {
            ctx.request_repaint_after(due - now);
            self.find_remote = Some((conversation, query, due));
            return;
        }
        self.backend.send(AccountCommand::SearchIn {
            conversation,
            query,
            remote: true,
        });
    }

    /// Enter in the find bar: Microsoft Search for the query now, if it is
    /// still waiting.
    pub(in crate::app) fn find_remote_now(&mut self, ctx: &egui::Context) {
        if let Some((_, _, due)) = &mut self.find_remote {
            *due = Instant::now();
        }
        self.send_due_find_search(Instant::now(), ctx);
    }

    /// Search over loaded messages, for the demo.
    pub(in crate::app) fn local_search(&self, query: &str) -> Vec<SearchHit> {
        let needle = query.to_lowercase();
        let mut hits: Vec<SearchHit> = self
            .conversations
            .iter()
            .flat_map(|(id, conversation)| {
                conversation
                    .messages
                    .iter()
                    .flat_map(|m| std::iter::once(m).chain(m.replies.iter()))
                    .filter(|m| !m.system && m.body.to_plain().to_lowercase().contains(&needle))
                    .map(move |m| SearchHit {
                        conversation: id.clone(),
                        message_id: m
                            .reply_to
                            .clone()
                            .filter(|_| id.is_channel())
                            .unwrap_or_else(|| m.id.clone()),
                        author: m.author.clone(),
                        snippet: m.body.summary(140),
                        at: m.created,
                    })
            })
            .collect();
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.at));
        hits.truncate(50);
        hits
    }
}
