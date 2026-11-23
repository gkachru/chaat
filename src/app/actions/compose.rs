//! App actions: compose.

use super::super::*;

impl App {
    pub(in crate::app) fn apply_start_reply(&mut self, message: String) {
        if let Some(open) = self.open.clone() {
            let draft = self.drafts.entry(open).or_default();
            draft.reply_to = Some(message);
            draft.editing = None;
            self.focus_composer = true;
        }
    }

    pub(in crate::app) fn apply_start_edit(&mut self, message: String) {
        if let Some(open) = self.open.clone() {
            let text = self
                .conversations
                .get(&open)
                .and_then(|c| c.find(&message))
                .map(|m| m.body.to_plain())
                .unwrap_or_default();
            self.drafts.insert(
                open,
                Draft {
                    text,
                    editing: Some(message),
                    ..Default::default()
                },
            );
            self.focus_composer = true;
        }
    }

    pub(in crate::app) fn apply_cancel_compose(&mut self) {
        if let Some(open) = self.open.clone()
            && let Some(draft) = self.drafts.get_mut(&open)
        {
            if draft.editing.take().is_some() {
                draft.text.clear();
            }
            draft.reply_to = None;
        }
    }

    pub(in crate::app) fn apply_attach_files(&mut self) {
        if self.open.is_none() || !self.ensure_scope(crate::auth::FILES_SCOPE) {
            return;
        }
        // The platform's own picker; it is modal, like a dialog.
        if let Some(paths) = rfd::FileDialog::new()
            .set_title("Attach files")
            .pick_files()
        {
            self.actions.push(Action::AddFiles(paths));
        }
    }

    pub(in crate::app) fn apply_paste(
        &mut self,
        conversation: ConversationId,
        range: (usize, usize),
        text: String,
    ) {
        self.drafts
            .entry(conversation.clone())
            .or_default()
            .pending_pastes += 1;
        self.backend.send(AccountCommand::Paste {
            conversation,
            range,
            text,
        });
    }

    pub(in crate::app) fn apply_remove_file(&mut self, index: usize) {
        if let Some(open) = self.open.clone()
            && let Some(draft) = self.drafts.get_mut(&open)
            && index < draft.files.len()
        {
            draft.files.remove(index);
        }
    }

    /// Saves the drafts that changed since the last call: at most one
    /// `SaveDraft` per conversation, none when nothing changed.
    pub fn sync_drafts(&mut self) {
        if self.demo {
            return;
        }
        let mut changed: Vec<ConversationId> = Vec::new();
        for (conversation, draft) in &self.drafts {
            // An edit in progress leaves the saved draft alone.
            if draft.editing.is_none() && !draft.is_saved(self.saved_drafts.get(conversation)) {
                changed.push(conversation.clone());
            }
        }
        for conversation in self.saved_drafts.keys() {
            if !self.drafts.contains_key(conversation) && !changed.contains(conversation) {
                changed.push(conversation.clone());
            }
        }
        for conversation in changed {
            let persisted = self.drafts.get(&conversation).and_then(Draft::persisted);
            match &persisted {
                Some(draft) => self
                    .saved_drafts
                    .insert(conversation.clone(), draft.clone()),
                None => self.saved_drafts.remove(&conversation),
            };
            self.backend.send(AccountCommand::SaveDraft {
                conversation,
                draft: persisted,
            });
        }
    }

    /// The chat list's line for a chat with an unsent draft: its text on one
    /// line.
    pub fn draft_preview(&self, chat: &str) -> Option<String> {
        let draft = self.drafts.get(&ConversationId::Chat(chat.to_owned()))?;
        let text = draft.text.split_whitespace().collect::<Vec<_>>().join(" ");
        (!text.is_empty()).then(|| text.chars().take(140).collect())
    }

    pub(in crate::app) fn add_files(&mut self, paths: Vec<PathBuf>) {
        let Some(open) = self.open.clone() else {
            self.toast(ToastKind::Info, "Open a chat or channel to send files to.");
            return;
        };
        if !self.ensure_scope(crate::auth::FILES_SCOPE) {
            return;
        }
        let mut refused = Vec::new();
        let draft = self.drafts.entry(open).or_default();
        for path in paths {
            let size = std::fs::metadata(&path).map(|m| (m.is_file(), m.len()));
            match size {
                Ok((true, size)) if size <= crate::graph::files::MAX_FILE => {
                    if !draft.files.contains(&path) {
                        draft.files.push(path);
                    }
                }
                _ => refused.push(
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            }
        }
        self.focus_composer = true;
        if !refused.is_empty() {
            self.toast(
                ToastKind::Error,
                format!(
                    "Not attached (folders and files over 1 GB are not sent): {}",
                    refused.join(", ")
                ),
            );
        }
    }

    pub(in crate::app) fn send(
        &mut self,
        conversation: ConversationId,
        text: String,
        reply_to: Option<String>,
        mentions: Vec<MentionRef>,
        files: Vec<PathBuf>,
        images: Vec<crate::clipboard::InlineImage>,
    ) {
        let text = text.trim_end().to_owned();
        if text.trim().is_empty() && files.is_empty() && images.is_empty() {
            return;
        }
        self.outdate_agent_proposals(&conversation);
        let (html, mentions) = html::compose_with_mentions(&text, &mentions);
        let mut preview_html = html.clone();
        for image in &images {
            preview_html.push_str(&format!(
                "<img src=\"{}\" width=\"{}\" height=\"{}\">",
                image.uri(),
                image.width,
                image.height
            ));
            self.media.insert(image.uri(), Image::Ready(image.uri()));
        }
        self.next_local += 1;
        let local_id = format!("local-{}", self.next_local);
        let me = self.me().to_owned();
        let author = self
            .profile
            .as_ref()
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| "You".into());
        let channel_reply = conversation.is_channel() && reply_to.is_some();
        let demo = self.demo;
        let mut message = Message {
            id: local_id.clone(),
            conversation: Some(conversation.clone()),
            reply_to: if channel_reply {
                reply_to.clone()
            } else {
                None
            },
            author_id: Some(me.clone()),
            author,
            created: crate::time::now(),
            modified: crate::time::now(),
            body: html::parse(&preview_html),
            delivery: if demo {
                Delivery::Sent
            } else {
                Delivery::Sending
            },
            ..Default::default()
        };
        for path in &files {
            message.attachments.push(crate::model::Attachment {
                id: path.display().to_string(),
                kind: crate::model::AttachmentKind::File,
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
                content_type: Some("reference".into()),
                url: None,
                content: None,
            });
        }
        if let (Some(quoted), false) = (&reply_to, conversation.is_channel())
            && let Some(original) = self
                .conversations
                .get(&conversation)
                .and_then(|c| c.find(quoted))
        {
            message.attachments.push(crate::model::Attachment {
                id: quoted.clone(),
                kind: crate::model::AttachmentKind::Quote {
                    message_id: quoted.clone(),
                    author: Some(original.author.clone()),
                    preview: original.body.summary(200),
                },
                name: None,
                content_type: Some("messageReference".into()),
                url: None,
                content: None,
            });
        }
        if demo {
            message.id = format!("demo-{}", self.next_local);
        }
        // Who can open the files: everyone in the chat. Channel uploads get
        // an organisation link instead (Graph does not list channel members
        // with Chaat's permissions).
        let recipients: Vec<String> = match &conversation {
            ConversationId::Chat(chat) => self
                .chat(chat)
                .map(|c| {
                    c.members
                        .iter()
                        .filter(|m| m.user_id != me)
                        .filter_map(|m| m.email.clone())
                        .collect()
                })
                .unwrap_or_default(),
            ConversationId::Channel { .. } => Vec::new(),
        };
        let entry = self.conversations.entry(conversation.clone()).or_default();
        if channel_reply {
            entry.layout.invalidate();
            if let Some(root) = &reply_to {
                entry.heights.remove(root);
            }
            if let Some(root) = entry
                .messages
                .iter_mut()
                .find(|m| Some(&m.id) == reply_to.as_ref())
            {
                root.replies.push(message.clone());
            }
            entry.expanded.insert(reply_to.clone().unwrap_or_default());
        } else {
            entry.messages.push(message.clone());
        }
        if let ConversationId::Chat(chat_id) = &conversation
            && let Some(chat) = self.chats.iter_mut().find(|c| &c.id == chat_id)
        {
            chat.preview = Some(crate::model::Preview {
                message_id: message.id.clone(),
                author_id: Some(me),
                author: Some(message.author.clone()),
                text: preview_text(&message),
                at: message.created,
                deleted: false,
                ..Default::default()
            });
            self.chats.sort_by_key(Chat::list_order);
        }
        self.drafts.remove(&conversation);
        if !channel_reply {
            self.scroll_to_end = 3;
        }
        if !demo {
            self.pending_sends.insert(
                local_id.clone(),
                PendingSend {
                    html: html.clone(),
                    reply_to: reply_to.clone(),
                    mentions: mentions.clone(),
                    files: files.clone(),
                    images: images.clone(),
                    recipients: recipients.clone(),
                },
            );
            let payload = crate::outbox::SendPayload {
                conversation,
                local_id,
                html,
                reply_to,
                mentions,
                files,
                images,
                recipients,
            };
            self.backend
                .send(AccountCommand::SaveOutbox(crate::outbox::OutboxEntry {
                    payload: payload.clone(),
                    created: message.created,
                    text: html::parse(&payload.html).to_plain(),
                }));
            self.backend.send(AccountCommand::Send(payload));
        }
    }
}
