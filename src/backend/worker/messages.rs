use super::*;

impl Worker {
    pub(super) fn load_older(&mut self, conversation: ConversationId, before: Option<i64>) {
        let Some(account) = &self.account else { return };
        if !self.freshness.claim(
            Resource::Messages(conversation.clone()),
            Trigger::History,
            Instant::now(),
        ) {
            return;
        }
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let links = Arc::clone(&self.older_links);
        tokio::spawn(async move {
            let resource = Resource::Messages(conversation.clone());
            out.send(AccountEvent::Loading(resource.clone(), true));
            let result = async {
                let page = match &conversation {
                    ConversationId::Chat(chat) => {
                        let cached = {
                            let store = store.lock().unwrap_or_else(|p| p.into_inner());
                            store
                                .messages_before(
                                    &conversation,
                                    before.unwrap_or(i64::MAX),
                                    CACHED_MESSAGES,
                                )
                                .unwrap_or_default()
                        };
                        if !cached.is_empty() {
                            remember_link(&links, &conversation, None);
                            out.send(AccountEvent::Messages {
                                conversation: conversation.clone(),
                                messages: cached,
                                merge: Merge::Older,
                                more: Some(true),
                                cached: true,
                            });
                            return Ok(None);
                        }
                        let link = links
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .get(&conversation)
                            .cloned();
                        let page = match link {
                            Some(link) => graph.chat_message_page(chat, &link).await?,
                            None => graph.chat_messages(chat, before).await?,
                        };
                        remember_link(&links, &conversation, page.older.clone());
                        page
                    }
                    ConversationId::Channel { team, channel } => {
                        let link = links
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .get(&conversation)
                            .cloned();
                        let Some(link) = link else {
                            return Ok(Some(false));
                        };
                        let page = graph.channel_messages(team, channel, Some(&link)).await?;
                        remember_link(&links, &conversation, page.older.clone());
                        page
                    }
                };
                let more = page.older.is_some() || page.messages.len() >= crate::graph::api::PAGE;
                save(&store, &conversation, &page.messages).await;
                out.send(AccountEvent::Messages {
                    conversation: conversation.clone(),
                    messages: page.messages,
                    merge: Merge::Older,
                    more: Some(more),
                    cached: false,
                });
                Ok::<_, GraphError>(None)
            }
            .await;
            out.send(AccountEvent::Loading(resource, false));
            match result {
                Ok(Some(more)) => out.send(AccountEvent::Messages {
                    conversation,
                    messages: Vec::new(),
                    merge: Merge::Older,
                    more: Some(more),
                    cached: false,
                }),
                Ok(None) => {}
                Err(error) => out.error("Loading older messages", &error),
            }
        });
    }

    pub(super) fn load_replies(&mut self, conversation: ConversationId, root: String) {
        let Some(account) = &self.account else { return };
        let ConversationId::Channel { team, channel } = &conversation else {
            return;
        };
        let (team, channel) = (team.clone(), channel.clone());
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.channel_replies(&team, &channel, &root).await {
                Ok(replies) => out.send(AccountEvent::Replies {
                    conversation,
                    root,
                    replies,
                }),
                Err(error) => out.error("Loading replies", &error),
            }
        });
    }

    pub(super) fn send(
        &mut self,
        conversation: ConversationId,
        local_id: String,
        mut message: Outgoing,
        reply_to: Option<String>,
        files: Vec<std::path::PathBuf>,
        recipients: Vec<String>,
    ) {
        let Some(account) = &self.account else {
            self.out.send(AccountEvent::SendFailed {
                conversation,
                local_id,
                reason: "Not signed in".into(),
            });
            return;
        };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        tokio::spawn(async move {
            // Files first: upload, share with the conversation, reference.
            for path in &files {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match share_file(&graph, &conversation, path, &recipients, &out).await {
                    Ok(shared) => message.files.push(shared),
                    Err(error) => {
                        log::warn!("upload failed: {error}");
                        out.send(AccountEvent::SendFailed {
                            conversation,
                            local_id,
                            reason: format!("uploading {name} failed ({error})"),
                        });
                        return;
                    }
                }
            }
            let result = match &conversation {
                ConversationId::Chat(chat) => match &reply_to {
                    Some(quoted) => graph.reply_with_quote(chat, quoted, &message).await,
                    None => graph.send_chat_message(chat, &message).await,
                },
                ConversationId::Channel { team, channel } => {
                    graph
                        .send_channel_message(team, channel, reply_to.as_deref(), &message)
                        .await
                }
            };
            match result {
                Ok(message) => {
                    if message.reply_to.is_none() {
                        save(&store, &conversation, std::slice::from_ref(&message)).await;
                    }
                    forget_outbox(&store, &local_id).await;
                    out.send(AccountEvent::Sent {
                        conversation,
                        local_id,
                        message: Box::new(message),
                    });
                }
                Err(error) => {
                    log::warn!("send failed: {error}");
                    out.send(AccountEvent::SendFailed {
                        conversation,
                        local_id,
                        reason: error.to_string(),
                    });
                }
            }
        });
    }

    /// Sends a kept message again, unless the conversation already shows it.
    pub(super) fn resend(&mut self, entry: crate::outbox::OutboxEntry) {
        let Some(account) = &self.account else {
            self.out.send(AccountEvent::SendFailed {
                conversation: entry.payload.conversation,
                local_id: entry.payload.local_id,
                reason: "Not signed in".into(),
            });
            return;
        };
        let resource = Resource::Messages(entry.payload.conversation.clone());
        let claimed = self
            .freshness
            .claim(resource.clone(), Trigger::Start, Instant::now());
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let internal = self.internal.clone();
        let record = account.record.clone();
        let links = Arc::clone(&self.older_links);
        let checked = Arc::clone(&self.resend_checked);
        tokio::spawn(async move {
            let mut checked = checked.lock().await;
            if !checked.contains(&entry.payload.conversation) {
                // Without a fresh look there is no telling whether Graph
                // took the message, so it is not sent blind.
                let fetched = claimed
                    && fetch_resource(&graph, &store, &out, &resource, &record, &links)
                        .await
                        .is_ok();
                if !fetched {
                    if claimed {
                        let _ = internal.send(Internal::FetchFailed(resource));
                    }
                    out.send(AccountEvent::SendFailed {
                        conversation: entry.payload.conversation,
                        local_id: entry.payload.local_id,
                        reason: "Chaat could not check whether it was sent. Retry to send it."
                            .into(),
                    });
                    return;
                }
                checked.insert(entry.payload.conversation.clone());
            }
            drop(checked);
            let me = record.user_id().to_owned();
            let recent = {
                let key = entry.payload.conversation.clone();
                let store = Arc::clone(&store);
                tokio::task::spawn_blocking(move || {
                    let store = store.lock().unwrap_or_else(|p| p.into_inner());
                    store.messages(&key, CACHED_MESSAGES).unwrap_or_default()
                })
                .await
                .unwrap_or_default()
            };
            match crate::outbox::already_sent(&entry, &recent, &me) {
                Some(message) => {
                    forget_outbox(&store, &entry.payload.local_id).await;
                    out.send(AccountEvent::Sent {
                        conversation: entry.payload.conversation,
                        local_id: entry.payload.local_id,
                        message: Box::new(message),
                    });
                }
                None => {
                    let _ = internal.send(Internal::Post(entry));
                }
            }
        });
    }

    pub(super) fn edit(&mut self, conversation: ConversationId, message: String, html: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            match graph.edit_message(&conversation, &message, &html).await {
                Ok(()) => {
                    let _ = internal.send(Internal::Refetch(conversation));
                }
                Err(error) => out.error("Editing the message", &error),
            }
        });
    }

    pub(super) fn delete(&mut self, conversation: ConversationId, message: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            match graph.delete_message(&me, &conversation, &message).await {
                Ok(()) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = store.delete_message(&conversation, &message);
                    }
                    out.send(AccountEvent::Deleted {
                        conversation,
                        message,
                    });
                }
                Err(error) => out.error("Deleting the message", &error),
            }
        });
    }

    pub(super) fn react(
        &mut self,
        conversation: ConversationId,
        message: String,
        emoji: String,
        on: bool,
    ) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            match graph.react(&conversation, &message, &emoji, on).await {
                Ok(()) => {
                    let _ = internal.send(Internal::Refetch(conversation));
                }
                Err(error) => out.error("Reacting", &error),
            }
        });
    }

    pub(super) fn cached_messages(&self, conversation: ConversationId) {
        let Some(account) = &self.account else { return };
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        tokio::spawn(async move {
            let key = conversation.clone();
            let messages = tokio::task::spawn_blocking(move || {
                store
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .messages(&key, CACHED_MESSAGES)
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            if !messages.is_empty() {
                out.send(AccountEvent::Messages {
                    conversation,
                    messages,
                    merge: Merge::Latest,
                    more: Some(true),
                    cached: true,
                });
            }
        });
    }

    /// Answers an agent from the cache. Never fetches.
    pub(super) fn agent_read(
        &self,
        plugin: String,
        request: u64,
        query: crate::plugin::agent::AgentQuery,
        access: crate::agents::access::AgentAccess,
    ) {
        let out = self.out.clone();
        let Some(account) = &self.account else {
            out.send(AccountEvent::AgentAnswer {
                plugin,
                request,
                data: Err("not_found"),
            });
            return;
        };
        let store = Arc::clone(&account.store);
        let me = account.record.user_id();
        let me = me.to_owned();
        tokio::spawn(async move {
            let data = super::super::agents::answer(store, me, query, access).await;
            out.send(AccountEvent::AgentAnswer {
                plugin,
                request,
                data,
            });
        });
    }
}

/// Drops a confirmed message from the outbox.
pub(super) async fn forget_outbox(store: &Arc<Mutex<Store>>, local_id: &str) {
    let store = Arc::clone(store);
    let local_id = local_id.to_owned();
    let _ = tokio::task::spawn_blocking(move || {
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let _ = store.delete_outbox(&local_id);
    })
    .await;
}

pub(super) async fn save(
    store: &Arc<Mutex<Store>>,
    conversation: &ConversationId,
    messages: &[Message],
) {
    if messages.is_empty() {
        return;
    }
    let store = Arc::clone(store);
    let conversation = conversation.clone();
    let messages = messages.to_vec();
    let _ = tokio::task::spawn_blocking(move || {
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(error) = store.save_messages(&conversation, &messages) {
            log::warn!("could not cache messages: {error}");
        }
    })
    .await;
}

pub(super) fn remember_link(
    links: &Arc<Mutex<HashMap<ConversationId, String>>>,
    conversation: &ConversationId,
    link: Option<String>,
) {
    let mut links = links.lock().unwrap_or_else(|p| p.into_inner());
    match link {
        Some(link) => {
            links.insert(conversation.clone(), link);
        }
        None => {
            links.remove(conversation);
        }
    }
}

/// Uploads one file, shares it with the conversation, and returns the
/// reference for the message. Sharing problems are reported but do not stop
/// the send: the people may already have access.
async fn share_file(
    graph: &Graph,
    conversation: &ConversationId,
    path: &std::path::Path,
    recipients: &[String],
    out: &Outbox,
) -> Result<SharedFile, GraphError> {
    let item = graph.upload_file(path).await?;
    let shared = match conversation {
        ConversationId::Chat(_) => graph.share_with(&item.id, recipients).await,
        ConversationId::Channel { .. } => graph.organization_link(&item.id).await,
    };
    if let Err(error) = shared {
        log::warn!("sharing an upload failed: {error}");
        out.send(AccountEvent::Error(format!(
            "{} was uploaded, but sharing it failed; some people may not be able to open it ({error})",
            item.name
        )));
    }
    let (Some(id), Some(url)) = (item.attachment_id(), item.content_url().map(str::to_owned))
    else {
        return Err(GraphError::Decode);
    };
    Ok(SharedFile {
        id,
        name: item.name,
        url,
    })
}

/// Draft saves waiting for the disk. Typing queues a save per frame; they
/// collapse to the latest one per conversation, and one blocking task writes
/// them, so the command loop never waits on SQLite.
type PendingDraft = (Arc<Mutex<Store>>, Option<crate::outbox::StoredDraft>);

#[derive(Default)]
pub(super) struct DraftQueue {
    pending: Mutex<HashMap<ConversationId, PendingDraft>>,
    running: std::sync::atomic::AtomicBool,
}

impl DraftQueue {
    pub(super) fn push(
        self: &Arc<Self>,
        store: &Arc<Mutex<Store>>,
        conversation: ConversationId,
        draft: Option<crate::outbox::StoredDraft>,
    ) {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(conversation, (Arc::clone(store), draft));
        if !self.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let queue = Arc::clone(self);
            tokio::task::spawn_blocking(move || queue.flush());
        }
    }

    fn flush(&self) {
        loop {
            let batch = {
                let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
                if pending.is_empty() {
                    // Cleared under the lock, so a push cannot slip between.
                    self.running
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
                std::mem::take(&mut *pending)
            };
            for (conversation, (store, draft)) in batch {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                if let Err(error) = store.save_draft(&conversation, draft.as_ref()) {
                    log::warn!("could not save a draft: {error}");
                }
            }
        }
    }
}

#[cfg(test)]
mod draft_queue_tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn latest_draft_per_conversation_wins() {
        let store = Arc::new(Mutex::new(Store::in_memory().unwrap()));
        let queue = Arc::new(DraftQueue::default());
        let chat = ConversationId::Chat("19:a".into());
        for n in 0..200 {
            queue.push(
                &store,
                chat.clone(),
                Some(crate::outbox::StoredDraft {
                    text: format!("t{n}"),
                    ..Default::default()
                }),
            );
        }
        while queue.running.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let drafts = store.lock().unwrap().drafts().unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].1.text, "t199");
        queue.push(&store, chat, None);
        while queue.running.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(store.lock().unwrap().drafts().unwrap().is_empty());
    }
}
