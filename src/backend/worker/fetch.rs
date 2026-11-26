//! Fetching from Graph for a resource the person looks at, and media.

use super::*;

impl Worker {
    pub(super) fn fetch(&mut self, resource: Resource, trigger: Trigger) {
        let Some(account) = &self.account else { return };
        if !self
            .freshness
            .claim(resource.clone(), trigger, Instant::now())
        {
            if trigger == Trigger::Push {
                log::debug!(
                    "push[{}]: {} skipped, fetched under a second ago",
                    account_tag(&account.record.id),
                    describe(&resource)
                );
            }
            return;
        }
        if trigger == Trigger::Push {
            log::debug!(
                "push[{}]: fetching {}",
                account_tag(&account.record.id),
                describe(&resource)
            );
        }
        let tag = account_tag(&account.record.id);
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let internal = self.internal.clone();
        let record = account.record.clone();
        let links = Arc::clone(&self.older_links);
        let reachable = Arc::clone(&self.reachable);
        tokio::spawn(async move {
            use std::sync::atomic::Ordering;
            out.send(AccountEvent::Loading(resource.clone(), true));
            let result = fetch_resource(&graph, &store, &out, &resource, &record, &links).await;
            if trigger == Trigger::Push {
                log::debug!(
                    "push[{tag}]: {} done (ok: {})",
                    describe(&resource),
                    result.is_ok()
                );
            }
            out.send(AccountEvent::Loading(resource.clone(), false));
            let lost = result.as_ref().err().is_some_and(is_unreachable);
            if lost == reachable.swap(!lost, Ordering::Relaxed) {
                out.send(AccountEvent::Reachable(!lost));
            }
            if result.is_ok() && resource == Resource::Chats {
                let me = record.user_id().to_owned();
                let people = contacts_of(&store, me, record.tenant_id.clone()).await;
                let _ = internal.send(Internal::Contacts { people, trigger });
            }
            if lost {
                // Brief outages fix themselves on the next look: the app
                // shows a quiet marker instead of an error.
                log::warn!("{}: Microsoft unreachable", describe(&resource));
                let _ = internal.send(Internal::FetchFailed(resource.clone()));
            } else if let Err(error) = result {
                let _ = internal.send(Internal::FetchFailed(resource.clone()));
                match (&resource, &error) {
                    (
                        Resource::Messages(ConversationId::Channel { .. }),
                        GraphError::Forbidden { .. },
                    ) => {
                        out.send(AccountEvent::ChannelAccess(Access::Forbidden));
                    }
                    (Resource::MyPresence | Resource::Calendar, GraphError::Forbidden { .. }) => {
                        log::info!("{resource:?} is not permitted for this account");
                    }
                    _ => out.error(describe(&resource), &error),
                }
                if let GraphError::Auth(TokenError::Rejected(reason)) = &error {
                    out.send(AccountEvent::Auth(AuthStatus::Expired {
                        username: String::new(),
                        reason: reason.clone(),
                    }));
                }
            }
        });
    }

    pub(super) fn media(&mut self, url: String) {
        let Some(account) = &self.account else { return };
        if !account.graph.is_graph_url(&url) {
            // Images hosted elsewhere are not fetched with credentials, and
            // not fetched at all: they could track who read the message.
            self.out.send(AccountEvent::Media { url, bytes: None });
            return;
        }
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let downloads = Arc::clone(&self.downloads);
        tokio::spawn(async move {
            let cache = Arc::clone(&store);
            let key = url.clone();
            let cached = tokio::task::spawn_blocking(move || {
                cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .media(&key)
                    .ok()
                    .flatten()
            })
            .await
            .ok()
            .flatten();
            if let Some(bytes) = cached {
                out.send(AccountEvent::Media {
                    url,
                    bytes: Some(bytes),
                });
                return;
            }
            let Ok(_permit) = downloads.acquire().await else {
                return;
            };
            let bytes = graph
                .get_bytes(&url, crate::graph::api::MEDIA_LIMIT)
                .await
                .ok();
            if let Some(bytes) = &bytes {
                let cache = Arc::clone(&store);
                let key = url.clone();
                let encoded = bytes.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = cache
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .save_media(&key, &encoded);
                })
                .await;
            }
            out.send(AccountEvent::Media { url, bytes });
        });
    }
}

/// A failure to reach Microsoft at all, as opposed to an answer refusing the
/// request.
fn is_unreachable(error: &GraphError) -> bool {
    matches!(
        error,
        GraphError::Network(_) | GraphError::Auth(TokenError::Unreachable(_))
    )
}

fn describe(resource: &Resource) -> &'static str {
    match resource {
        Resource::Profile => "Loading your profile",
        Resource::Chats => "Loading chats",
        Resource::Messages(_) => "Loading messages",
        Resource::Teams => "Loading teams",
        Resource::Channels(_) => "Loading channels",
        Resource::Calendar => "Loading the calendar",
        Resource::MyPresence => "Loading your status",
        Resource::OutOfOffice => "Loading your automatic replies",
        Resource::Members(_) => "Loading the people in this chat",
        Resource::Pins(_) => "Loading the pinned messages",
    }
}

pub(super) async fn fetch_resource(
    graph: &Arc<Graph>,
    store: &Arc<Mutex<Store>>,
    out: &Outbox,
    resource: &Resource,
    record: &AccountRecord,
    links: &Arc<Mutex<HashMap<ConversationId, String>>>,
) -> Result<(), GraphError> {
    match resource {
        Resource::Profile => {
            let profile = graph.me(Some(record.tenant_id.clone())).await?;
            out.send(AccountEvent::Profile(Box::new(profile)));
        }
        Resource::Chats => {
            let mut chats = graph.chats(2).await?;
            // The notes chat is not in the listing; a failure to find it
            // leaves the rest of the list as it is.
            let me = crate::model::Member {
                tenant_id: None,
                user_id: record.user_id().to_owned(),
                display_name: record.display_name.clone(),
                email: Some(record.username.clone()).filter(|u| !u.is_empty()),
            };
            match graph.notes_chat(me).await {
                Ok(Some(notes)) => chats.insert(0, notes),
                Ok(None) => {}
                Err(error) => log::info!("notes chat unavailable: {error}"),
            }
            {
                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                store.freshen_previews(&mut chats);
                if let Err(error) = store.save_chats(&chats) {
                    log::warn!("could not cache chats: {error}");
                }
            }
            out.send(AccountEvent::Chats {
                chats,
                cached: false,
            });
        }
        Resource::Messages(conversation) => {
            // What the cache has, at once.
            let (cached, since) = {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                (
                    store
                        .messages(conversation, CACHED_MESSAGES)
                        .unwrap_or_default(),
                    store.last_modified(conversation),
                )
            };
            let had_cache = !cached.is_empty();
            if had_cache {
                remember_link(links, conversation, None);
                out.send(AccountEvent::Messages {
                    conversation: conversation.clone(),
                    messages: cached,
                    merge: Merge::Latest,
                    // A date-ranged refresh below cannot establish whether
                    // history before the cache exists. Keep paging available.
                    more: Some(true),
                    cached: true,
                });
            }
            match conversation {
                ConversationId::Chat(chat) => {
                    // A date-ranged refresh when the cache knows where it
                    // stopped; the newest page otherwise.
                    if let Some(since) = since.filter(|_| had_cache) {
                        let changes = graph.chat_changes(chat, since - 1_000).await?;
                        if changes.len() < crate::graph::api::PAGE * 4 {
                            save(store, conversation, &changes).await;
                            out.send(AccountEvent::Messages {
                                conversation: conversation.clone(),
                                messages: changes,
                                merge: Merge::Changes,
                                more: None,
                                cached: false,
                            });
                            return Ok(());
                        }
                    }
                    let page = graph.chat_messages(chat, None).await?;
                    remember_link(links, conversation, page.older.clone());
                    let more =
                        page.older.is_some() || page.messages.len() >= crate::graph::api::PAGE;
                    save(store, conversation, &page.messages).await;
                    out.send(AccountEvent::Messages {
                        conversation: conversation.clone(),
                        messages: page.messages,
                        merge: Merge::Latest,
                        more: Some(more),
                        cached: false,
                    });
                }
                ConversationId::Channel { team, channel } => {
                    let page = graph.channel_messages(team, channel, None).await?;
                    out.send(AccountEvent::ChannelAccess(Access::Granted));
                    remember_link(links, conversation, page.older.clone());
                    save(store, conversation, &page.messages).await;
                    out.send(AccountEvent::Messages {
                        conversation: conversation.clone(),
                        messages: page.messages,
                        merge: Merge::Latest,
                        more: Some(page.older.is_some()),
                        cached: false,
                    });
                }
            }
        }
        Resource::Teams => {
            let teams = graph.joined_teams().await?;
            {
                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                let _ = store.save_teams(&teams);
            }
            out.send(AccountEvent::Teams {
                teams: teams.clone(),
                cached: false,
            });
            // Channels of every team, a few at a time.
            let limit = Arc::new(Semaphore::new(3));
            let mut tasks = Vec::new();
            for team in teams {
                let graph = Arc::clone(graph);
                let store = Arc::clone(store);
                let out = out.clone();
                let limit = Arc::clone(&limit);
                tasks.push(tokio::spawn(async move {
                    let Ok(_permit) = limit.acquire().await else {
                        return;
                    };
                    match graph.channels(&team.id).await {
                        Ok(channels) => {
                            {
                                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                                let _ = store.save_channels(&team.id, &channels);
                            }
                            out.send(AccountEvent::Channels {
                                team: Some(team.id),
                                channels,
                            });
                        }
                        Err(error) => log::info!("channels unavailable for a team: {error}"),
                    }
                }));
            }
            for task in tasks {
                let _ = task.await;
            }
        }
        Resource::Channels(team) => {
            let channels = graph.channels(team).await?;
            {
                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                let _ = store.save_channels(team, &channels);
            }
            out.send(AccountEvent::Channels {
                team: Some(team.clone()),
                channels,
            });
        }
        Resource::Calendar => {
            let from = crate::time::start_of_day(crate::time::now());
            let to = from + 14 * 24 * 60 * 60 * 1000;
            let meetings = graph.calendar(from, to).await?;
            {
                let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
                let _ = store.save_meetings(&meetings);
            }
            out.send(AccountEvent::Meetings {
                meetings,
                cached: false,
            });
        }
        Resource::MyPresence => {
            let presence = graph.my_presence().await?;
            out.send(AccountEvent::MyPresence(presence));
        }
        Resource::Pins(chat) => {
            let me = record.user_id();
            let pins = graph.pinned_messages(chat, me).await?;
            out.send(AccountEvent::Pins {
                chat: chat.clone(),
                pins,
            });
        }
        Resource::Members(chat) => {
            let members: Vec<crate::model::Member> = graph
                .chat_members(chat)
                .await?
                .into_iter()
                .map(|(_, member)| member)
                .collect();
            {
                // The cached chat keeps the whole list, not the first 25.
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(mut cached) = store
                    .chats()
                    .ok()
                    .and_then(|chats| chats.into_iter().find(|c| &c.id == chat))
                {
                    cached.members = members.clone();
                    let _ = store.upsert_chat(&cached);
                }
            }
            out.send(AccountEvent::Members {
                chat: chat.clone(),
                members,
            });
        }
        Resource::OutOfOffice => match graph.out_of_office().await {
            Ok(value) => out.send(AccountEvent::OutOfOffice(value)),
            Err(error) if crate::graph::mailbox::no_mailbox(&error) => {
                out.send(AccountEvent::OutOfOfficeUnavailable);
            }
            Err(error) => return Err(error),
        },
    }
    Ok(())
}
