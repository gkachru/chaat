//! App events: messages.

use super::super::*;

impl App {
    pub(in crate::app) fn handle_pasted(
        &mut self,
        conversation: ConversationId,
        range: (usize, usize),
        text: String,
        image: Result<Option<crate::clipboard::InlineImage>, &'static str>,
        ctx: &egui::Context,
    ) {
        let draft = self.drafts.entry(conversation.clone()).or_default();
        draft.pending_pastes = draft.pending_pastes.saturating_sub(1);
        match image {
            Ok(Some(image)) if draft.editing.is_none() => {
                if draft.images.iter().map(|i| i.png.len()).sum::<usize>() + image.png.len()
                    > crate::clipboard::MAX_BYTES
                {
                    self.toast(
                        ToastKind::Error,
                        "The pasted images are too large for one message. Send them separately.",
                    );
                } else {
                    crate::images::include(ctx, image.uri(), image.png.clone());
                    draft.images.push(image);
                }
            }
            Ok(Some(_)) => self.toast(
                ToastKind::Info,
                "Finish editing before pasting an image into a new message.",
            ),
            result => {
                if !text.is_empty() {
                    let chars: Vec<char> = draft.text.chars().collect();
                    let start = range.0.min(chars.len());
                    let end = range.1.min(chars.len()).max(start);
                    draft.text = format!(
                        "{}{}{}",
                        chars[..start].iter().collect::<String>(),
                        text,
                        chars[end..].iter().collect::<String>()
                    );
                    draft.paste_cursor = Some(start + text.chars().count());
                } else if let Err(error) = result {
                    self.toast(ToastKind::Error, error);
                }
            }
        }
    }

    pub(in crate::app) fn handle_replies(
        &mut self,
        conversation: ConversationId,
        root: String,
        replies: Vec<Message>,
    ) {
        let entry = self.conversations.entry(conversation).or_default();
        entry.layout.invalidate();
        entry.heights.remove(&root);
        if let Some(root) = entry.messages.iter_mut().find(|m| m.id == root) {
            // Replies still sending, or failed with Retry, stay after the
            // ones Graph has.
            let unsent: Vec<Message> = std::mem::take(&mut root.replies)
                .into_iter()
                .filter(|r| r.delivery != Delivery::Sent && !replies.iter().any(|n| n.id == r.id))
                .collect();
            root.replies = replies;
            root.replies.extend(unsent);
        }
    }

    pub(in crate::app) fn handle_send_failed(
        &mut self,
        conversation: ConversationId,
        local_id: String,
        reason: String,
    ) {
        if let Some(entry) = self.conversations.get_mut(&conversation)
            && let Some(message) = entry.find_mut(&local_id)
        {
            message.delivery = Delivery::Failed;
        }
        self.toast(ToastKind::Error, format!("Not sent: {reason}"));
    }

    pub(in crate::app) fn handle_deleted(&mut self, conversation: ConversationId, message: String) {
        if let Some(entry) = self.conversations.get_mut(&conversation)
            && let Some(message) = entry.find_mut(&message)
        {
            message.deleted = true;
            message.body = html::RichText::default();
        }
    }

    pub(in crate::app) fn handle_search_results(
        &mut self,
        query: String,
        hits: Vec<SearchHit>,
        remote: bool,
    ) {
        if !self.hidden && query == self.switcher.searched {
            if remote {
                self.switcher.remote_hits = hits;
            } else {
                self.switcher.hits = hits;
            }
        }
    }

    pub(in crate::app) fn on_messages(
        &mut self,
        conversation: ConversationId,
        messages: Vec<Message>,
        merge: Merge,
        more: Option<bool>,
        cached: bool,
    ) {
        self.after_messages_pins(&conversation, &messages);
        if !self.recent_conversations.contains(&conversation) {
            self.recent_conversations.push_front(conversation.clone());
        }
        let me = self.me().to_owned();
        let open = !self.hidden && self.open.as_ref() == Some(&conversation);
        let focused = self.window_focused;
        let entry = self.conversations.entry(conversation.clone()).or_default();
        // The disk-only reopen can finish after Graph. Do not replace a
        // newer network answer with a snapshot read before that answer.
        if cached && merge == Merge::Latest && entry.fresh {
            return;
        }
        let newest_before = entry
            .messages
            .iter()
            .filter(|m| m.delivery == Delivery::Sent)
            .map(|m| m.created)
            .max();
        let fresh_from_others: Vec<&Message> = messages
            .iter()
            .filter(|m| !m.system && !m.deleted && !m.is_from(&me))
            .filter(|m| newest_before.is_some_and(|newest| m.created > newest))
            .collect();
        let incoming_new: Vec<NoticeItem> = fresh_from_others
            .iter()
            .map(|m| NoticeItem {
                message: m.id.clone(),
                author: Some(m.author.clone()),
                text: m.body.summary(140),
                mentions_me: m.mentions(&me),
                urgent: m.urgent,
                created: m.created,
            })
            .collect();
        let agent_new: Vec<Message> = if cached || merge == Merge::Older {
            Vec::new()
        } else {
            fresh_from_others.into_iter().cloned().collect()
        };
        if merge == Merge::Older && !messages.is_empty() {
            entry.restore_history_anchor = true;
        }
        entry.merge(messages, merge);
        if let Some(more) = more {
            // A cached page says history may exist. It must not undo an
            // end-of-history answer Graph already gave in this process.
            if !cached || !entry.fresh {
                entry.more = more;
            }
        }
        if !cached {
            entry.fresh = true;
        }
        self.agent_events_new_messages(&conversation, &agent_new);
        if !cached && !incoming_new.is_empty() {
            self.notify_batch(&conversation, incoming_new);
        }
        // A message that arrives while Chaat is behind another window stays
        // unread, so Teams keeps it unread on other devices too. Coming back
        // to the window marks it read.
        if open
            && focused
            && !cached
            && let ConversationId::Chat(chat) = &conversation
        {
            self.mark_read(&chat.clone());
        }
        // Keep the chat list's preview in step with what the chat shows.
        // People added or removed don't count, as in Teams: they would move
        // the chat up the list.
        if let ConversationId::Chat(chat_id) = &conversation
            && let Some(last) = self
                .conversations
                .get(&conversation)
                .and_then(|c| c.messages.iter().rev().find(|m| m.membership.is_none()))
                .cloned()
            && let Some(chat) = self.chats.iter_mut().find(|c| &c.id == chat_id)
            && chat
                .preview
                .as_ref()
                .is_none_or(|p| p.at < last.created || p.membership.is_some())
            && last.delivery == Delivery::Sent
        {
            chat.preview = Some(crate::model::Preview {
                message_id: last.id.clone(),
                author_id: last.author_id.clone(),
                author: Some(last.author.clone()),
                text: preview_text(&last),
                at: last.created,
                deleted: last.deleted,
                system: last.system,
                membership: last.membership.clone(),
            });
            self.chats.sort_by_key(Chat::list_order);
        }
        self.trim_conversations();
    }

    /// Drafts and unsent messages from before the last exit. Young unsent
    /// messages go out again (after the backend checks they were not
    /// delivered); older ones show as failed with Retry and Delete.
    pub(in crate::app) fn on_restored(
        &mut self,
        drafts: Vec<(ConversationId, crate::outbox::StoredDraft)>,
        outbox: Vec<crate::outbox::OutboxEntry>,
    ) {
        for (conversation, saved) in drafts {
            let empty = self
                .drafts
                .get(&conversation)
                .is_none_or(|d| d.text.trim().is_empty());
            if empty {
                self.drafts.insert(
                    conversation.clone(),
                    Draft {
                        text: saved.text.clone(),
                        reply_to: saved.reply_to.clone(),
                        mentions: saved.mentions.clone(),
                        ..Default::default()
                    },
                );
            }
            self.saved_drafts.insert(conversation, saved);
        }
        let now = crate::time::now();
        for entry in outbox {
            if self.pending_sends.contains_key(&entry.payload.local_id) {
                // The same start reported twice: it is already shown and in
                // flight, and sending it again could post it twice.
                continue;
            }
            if let Some(number) = entry
                .payload
                .local_id
                .strip_prefix("local-")
                .and_then(|n| n.parse::<u64>().ok())
            {
                self.next_local = self.next_local.max(number);
            }
            let conversation = entry.payload.conversation.clone();
            let channel_reply = conversation.is_channel() && entry.payload.reply_to.is_some();
            // The duplicate check reads the conversation's top-level
            // messages, which cannot show a thread reply: those are not sent
            // again on their own.
            let resend = entry.resendable(now) && !channel_reply;
            let known = self.conversations.contains_key(&conversation);
            let mut preview_html = entry.payload.html.clone();
            for image in &entry.payload.images {
                preview_html.push_str(&format!(
                    "<img src=\"{}\" width=\"{}\" height=\"{}\">",
                    image.uri(),
                    image.width,
                    image.height
                ));
                self.media.insert(image.uri(), Image::Ready(image.uri()));
            }
            let mut message = Message {
                id: entry.payload.local_id.clone(),
                conversation: Some(conversation.clone()),
                reply_to: if channel_reply {
                    entry.payload.reply_to.clone()
                } else {
                    None
                },
                author_id: Some(self.me().to_owned()),
                author: self
                    .profile
                    .as_ref()
                    .map(|p| p.display_name.clone())
                    .unwrap_or_else(|| "You".into()),
                created: entry.created,
                modified: entry.created,
                body: html::parse(&preview_html),
                delivery: if resend {
                    Delivery::Sending
                } else {
                    Delivery::Failed
                },
                ..Default::default()
            };
            for path in &entry.payload.files {
                message.attachments.push(crate::model::Attachment {
                    id: path.display().to_string(),
                    kind: crate::model::AttachmentKind::File,
                    name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
                    content_type: Some("reference".into()),
                    url: None,
                    content: None,
                });
            }
            let view = self.conversations.entry(conversation.clone()).or_default();
            view.layout.invalidate();
            let root = channel_reply
                .then(|| {
                    view.messages
                        .iter()
                        .position(|m| Some(&m.id) == entry.payload.reply_to.as_ref())
                })
                .flatten();
            if let Some(root) = root {
                if view.find(&entry.payload.local_id).is_none() {
                    view.messages[root].replies.push(message);
                }
            } else if view.find(&entry.payload.local_id).is_none() {
                // A reply whose thread root is not loaded yet shows as its own
                // row, so a failed one can still be retried or deleted.
                message.reply_to = None;
                view.messages.push(message);
                view.messages.sort_by(|a, b| {
                    (a.delivery != Delivery::Sent, a.created, &a.id).cmp(&(
                        b.delivery != Delivery::Sent,
                        b.created,
                        &b.id,
                    ))
                });
            }
            if !known && !self.demo {
                // Opening later would skip the cache for a view that exists.
                self.backend
                    .send(AccountCommand::CachedMessages(conversation));
            }
            self.pending_sends.insert(
                entry.payload.local_id.clone(),
                PendingSend {
                    html: entry.payload.html.clone(),
                    reply_to: entry.payload.reply_to.clone(),
                    mentions: entry.payload.mentions.clone(),
                    files: entry.payload.files.clone(),
                    images: entry.payload.images.clone(),
                    recipients: entry.payload.recipients.clone(),
                },
            );
            if resend && !self.demo {
                self.backend.send(AccountCommand::Resend(entry));
            }
        }
    }

    /// Hits for the find bar. Hits for an older query, or for a conversation
    /// that is no longer open, are dropped.
    pub(in crate::app) fn on_conversation_hits(
        &mut self,
        conversation: ConversationId,
        query: String,
        mut hits: Vec<SearchHit>,
    ) {
        if self.hidden || self.open.as_ref() != Some(&conversation) {
            return;
        }
        let Some(find) = &mut self.find else { return };
        if query != find.searched {
            return;
        }
        hits.retain(|hit| hit.conversation == conversation);
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.at));
        let shown = find.hits.get(find.current).map(|h| h.message_id.clone());
        let first = find.hits.is_empty();
        find.current = shown
            .and_then(|id| hits.iter().position(|h| h.message_id == id))
            .unwrap_or(0);
        find.hits = hits;
        if first && let Some(hit) = find.hits.first() {
            self.jump_to = Some(hit.message_id.clone());
        }
    }

    pub(in crate::app) fn on_sent(
        &mut self,
        conversation: ConversationId,
        local_id: String,
        message: Message,
    ) {
        self.pending_sends.remove(&local_id);
        self.agent_event_user_sent(&conversation, &message.id);
        let entry = self.conversations.entry(conversation.clone()).or_default();
        if let Some(reply_to) = &message.reply_to
            && conversation.is_channel()
        {
            entry.messages.retain(|m| m.id != local_id);
            if let Some(root) = entry.messages.iter_mut().find(|m| &m.id == reply_to) {
                root.replies.retain(|r| r.id != local_id);
                entry.heights.remove(reply_to);
                entry.layout.invalidate();
                root.replies.push(message);
            }
            return;
        }
        entry.messages.retain(|m| m.id != local_id);
        entry.merge(vec![message], Merge::Changes);
        self.fetch(Resource::Messages(conversation), Trigger::AfterAction);
    }
}
