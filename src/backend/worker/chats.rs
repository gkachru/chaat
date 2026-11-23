use super::*;

impl Worker {
    pub(super) fn create_chat(&mut self, others: Vec<String>, topic: Option<String>) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            match graph.create_chat(&me, &others, topic.as_deref()).await {
                Ok(chat) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = store.upsert_chat(&chat);
                    }
                    out.send(AccountEvent::ChatCreated(Box::new(chat)));
                }
                Err(error) => out.error("Starting the chat", &error),
            }
        });
    }

    pub(super) fn add_members(&mut self, chat: String, users: Vec<String>, history: bool) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            let mut added = Vec::new();
            let mut failed = Vec::new();
            for user in users {
                match graph.add_chat_member(&chat, &user, history).await {
                    Ok(()) => added.push(user),
                    Err(error) => {
                        log::info!("adding a person to a chat failed: {error}");
                        failed.push(user);
                    }
                }
            }
            out.send(AccountEvent::MembersAdded {
                chat: chat.clone(),
                added,
                failed,
            });
            // Read the list back, so what is shown is what Graph has.
            let _ = internal.send(Internal::Confirm(Resource::Members(chat)));
        });
    }

    pub(super) fn leave_chat(&mut self, chat: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            // The membership id is read now, never kept: it changes when
            // the person is removed and added again.
            let result = async {
                let mine = graph
                    .chat_members(&chat)
                    .await?
                    .into_iter()
                    .find(|(_, member)| member.user_id == me)
                    .map(|(membership, _)| membership);
                match mine {
                    Some(membership) => graph.remove_chat_member(&chat, &membership).await,
                    None => Err(GraphError::Decode),
                }
            }
            .await;
            match result {
                Ok(()) => {
                    {
                        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                        if let Ok(mut chats) = store.chats() {
                            chats.retain(|c| c.id != chat);
                            let _ = store.save_chats(&chats);
                        }
                    }
                    out.send(AccountEvent::LeftChat(chat));
                }
                Err(error) => {
                    log::info!("leaving a chat failed: {error}");
                    out.send(AccountEvent::LeaveFailed {
                        chat,
                        admin: matches!(error, GraphError::Forbidden { .. }),
                    });
                }
            }
        });
    }

    pub(super) fn find_meeting_chat(&mut self, join_url: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        tokio::spawn(async move {
            let chat = match graph.meeting_chat(&join_url).await {
                Ok(Some(chat)) => {
                    let store = store.lock().unwrap_or_else(|p| p.into_inner());
                    let _ = store.upsert_chat(&chat);
                    Some(Box::new(chat))
                }
                Ok(None) => None,
                Err(error) => {
                    log::info!("meeting chat lookup failed: {error}");
                    None
                }
            };
            out.send(AccountEvent::MeetingChat { join_url, chat });
        });
    }

    pub(super) fn mark_read(&mut self, chat: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let me = account.record.user_id().to_owned();
        let tenant = account.record.tenant_id.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.mark_chat_read(&chat, &me, &tenant).await {
                log::info!("could not mark a chat read: {error}");
            }
        });
    }

    pub(super) fn hide_chat(&mut self, chat: String, hide: bool) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        let tenant = account.record.tenant_id.clone();
        tokio::spawn(async move {
            match graph.hide_chat(&chat, &me, &tenant, hide).await {
                Ok(()) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(mut cached) = store
                            .chats()
                            .ok()
                            .and_then(|chats| chats.into_iter().find(|c| c.id == chat))
                        {
                            cached.hidden = hide;
                            let _ = store.upsert_chat(&cached);
                        }
                    }
                    out.send(AccountEvent::ChatHidden {
                        chat,
                        hidden: hide,
                        ok: true,
                    });
                }
                Err(error) => {
                    log::info!("hiding a chat failed: {error}");
                    out.send(AccountEvent::ChatHidden {
                        chat,
                        hidden: hide,
                        ok: false,
                    });
                }
            }
        });
    }

    pub(super) fn mark_chat_unread(&mut self, chat: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        let tenant = account.record.tenant_id.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.mark_chat_unread(&chat, &me, &tenant).await {
                log::info!("could not mark a chat unread: {error}");
                out.send(AccountEvent::MarkUnreadFailed(chat));
            }
        });
    }

    pub(super) fn pin_message(&mut self, chat: String, message: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.pin_message(&chat, &message).await {
                out.error("Pinning", &error);
            }
            // Success or not, read the pins back so the banner is true.
            let _ = internal.send(Internal::Confirm(Resource::Pins(chat)));
        });
    }

    pub(super) fn unpin_message(&mut self, chat: String, pin: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.unpin_message(&chat, &pin).await {
                out.error("Unpinning", &error);
            }
            let _ = internal.send(Internal::Confirm(Resource::Pins(chat)));
        });
    }

    pub(super) fn find_meeting_by_id(&mut self, id: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            let join_url = match graph.meeting_join_url_by_id(&id).await {
                Ok(url) => url,
                Err(error) => {
                    log::info!("meeting ID lookup failed: {error}");
                    None
                }
            };
            out.send(AccountEvent::MeetingById { id, join_url });
        });
    }
}
