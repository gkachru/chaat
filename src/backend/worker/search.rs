use super::*;

impl Worker {
    pub(super) fn search_in(&mut self, conversation: ConversationId, query: String, remote: bool) {
        let Some(account) = &self.account else { return };
        let store = Arc::clone(&account.store);
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            let local = {
                let store = Arc::clone(&store);
                let conversation = conversation.clone();
                let query = query.clone();
                tokio::task::spawn_blocking(move || {
                    let store = store.lock().unwrap_or_else(|p| p.into_inner());
                    store
                        .search_in(&conversation, &query, 200)
                        .unwrap_or_default()
                })
                .await
                .unwrap_or_default()
            };
            if !remote {
                out.send(AccountEvent::ConversationHits {
                    conversation,
                    query,
                    hits: local,
                });
                return;
            }
            if query.trim().chars().count() < 3 {
                return;
            }
            match graph.search_messages(&query).await {
                Ok(remote) => out.send(AccountEvent::ConversationHits {
                    hits: conversation_hits(&conversation, &local, remote),
                    conversation,
                    query,
                }),
                Err(error) => log::info!("Microsoft Search unavailable: {error}"),
            }
        });
    }

    pub(super) fn search(&mut self, query: String) {
        let Some(account) = &self.account else { return };
        let store = Arc::clone(&account.store);
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let latest = Arc::clone(&self.searches);
        let mine = next_search(&latest);
        tokio::spawn(async move {
            let local = {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                store.search(&query, 50).unwrap_or_default()
            };
            out.send(AccountEvent::SearchResults {
                query: query.clone(),
                hits: local,
                remote: false,
            });
            if query.trim().chars().count() < 3 || !still_latest(&latest, mine).await {
                return;
            }
            match graph.search_messages(&query).await {
                Ok(hits) => out.send(AccountEvent::SearchResults {
                    query,
                    hits,
                    remote: true,
                }),
                Err(error) => log::info!("Microsoft Search unavailable: {error}"),
            }
        });
    }
}
