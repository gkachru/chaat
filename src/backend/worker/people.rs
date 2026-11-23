use super::*;

impl Worker {
    pub(super) fn search_people(&mut self, query: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let latest = Arc::clone(&self.people_searches);
        let mine = next_search(&latest);
        tokio::spawn(async move {
            if !still_latest(&latest, mine).await {
                return;
            }
            match graph.search_people(&query).await {
                Ok(people) => out.send(AccountEvent::People { query, people }),
                Err(error) => log::info!("people search unavailable: {error}"),
            }
        });
    }

    /// Looks up a name once per person every ten minutes, like photos.
    pub(super) fn user_name(&mut self, user: String) {
        let Some(account) = &self.account else { return };
        let now = Instant::now();
        if self
            .names_asked
            .get(&user)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(600))
        {
            return;
        }
        self.names_asked.insert(user.clone(), now);
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.user_name(&user).await {
                Ok(name) => out.send(AccountEvent::UserName { user, name }),
                Err(error) => log::debug!("name unavailable: {error}"),
            }
        });
    }

    pub(super) fn photo(&mut self, user: String) {
        let Some(account) = &self.account else { return };
        let now = Instant::now();
        if self
            .photos_asked
            .get(&user)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(600))
        {
            return;
        }
        self.photos_asked.insert(user.clone(), now);
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let downloads = Arc::clone(&self.downloads);
        tokio::spawn(async move {
            let cached = {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                store.photo(&user, PHOTO_MAX_AGE)
            };
            if let Some(bytes) = cached {
                out.send(AccountEvent::Photo { user, bytes });
                return;
            }
            let Ok(_permit) = downloads.acquire().await else {
                return;
            };
            match graph.photo(&user).await {
                Ok(bytes) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = store.save_photo(&user, bytes.as_deref());
                    }
                    out.send(AccountEvent::Photo { user, bytes });
                }
                Err(error) => log::debug!("photo unavailable: {error}"),
            }
        });
    }
}
