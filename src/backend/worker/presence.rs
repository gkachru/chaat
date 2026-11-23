use super::*;

impl Worker {
    pub(super) fn set_availability(&mut self, availability: Option<crate::model::Availability>) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            match graph.set_preferred_presence(&me, availability).await {
                Ok(()) => {
                    if let Ok(presence) = graph.my_presence().await {
                        out.send(AccountEvent::MyPresence(presence));
                    }
                }
                Err(error) => out.error("Setting your status", &error),
            }
        });
    }

    pub(super) fn set_status_message(
        &mut self,
        text: String,
        expires: Option<crate::model::Millis>,
    ) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let me = account.record.user_id().to_owned();
        tokio::spawn(async move {
            match graph.set_status_message(&me, &text, expires).await {
                Ok(()) => {
                    if let Ok(presence) = graph.my_presence().await {
                        out.send(AccountEvent::MyPresence(presence));
                    }
                }
                Err(error) => out.error("Setting your status message", &error),
            }
        });
    }

    /// Saves the automatic replies, shows them as saved, and reads them back.
    pub(super) fn set_out_of_office(&mut self, value: crate::model::OutOfOffice) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let internal = self.internal.clone();
        tokio::spawn(async move {
            match graph.set_out_of_office(&value).await {
                Ok(()) => {
                    out.send(AccountEvent::OutOfOffice(value));
                    let _ = internal.send(Internal::Confirm(Resource::OutOfOffice));
                }
                Err(error) => {
                    if crate::graph::mailbox::no_mailbox(&error) {
                        out.send(AccountEvent::OutOfOfficeUnavailable);
                    } else {
                        out.error("Saving your automatic replies", &error);
                    }
                    out.send(AccountEvent::OutOfOfficeNotSaved);
                }
            }
        });
    }

    /// The person is using Chaat. An Available presence session turns
    /// inactive five minutes after its last refresh and Away five minutes
    /// later, so Chaat refreshes it only while someone is at the keyboard;
    /// stepping away lets Graph mark them Away, as a Teams client would.
    pub(super) fn presence_active(&mut self) {
        let Some(account) = &self.account else { return };
        let (Some(config), true) = (&self.config, self.session_enabled) else {
            return;
        };
        if !account
            .graph
            .session()
            .has_scope(auth::PRESENCE_WRITE_SCOPE)
        {
            return;
        }
        let now = Instant::now();
        if self
            .session_refreshed
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(4 * 60))
        {
            return;
        }
        self.session_refreshed = Some(now);
        let graph = Arc::clone(&account.graph);
        let me = account.record.user_id().to_owned();
        let client = config.client_id.clone();
        tokio::spawn(async move {
            if let Err(error) = graph.set_presence_session(&me, &client, false, 60).await {
                log::info!("could not refresh the presence session: {error}");
            }
        });
    }

    /// Ends Chaat's presence session, if it started one (at sign-out, when
    /// turned off, and at exit).
    pub(super) async fn end_presence_session(&mut self) {
        if self.session_refreshed.take().is_none() {
            return;
        }
        let (Some(account), Some(config)) = (&self.account, &self.config) else {
            return;
        };
        let me = account.record.user_id().to_owned();
        let clear = account.graph.clear_presence_session(&me, &config.client_id);
        if let Ok(Err(error)) = tokio::time::timeout(Duration::from_secs(3), clear).await {
            log::info!("could not end the presence session: {error}");
        }
    }

    pub(super) fn presence(&mut self, users: Vec<String>) {
        let Some(account) = &self.account else { return };
        if !account.graph.session().has_scope("Presence.Read.All") {
            return;
        }
        let now = Instant::now();
        let due: Vec<String> = users
            .into_iter()
            .filter(|user| {
                self.presence_seen
                    .get(user)
                    .is_none_or(|at| now.duration_since(*at) >= PRESENCE_REUSE)
            })
            .collect();
        if due.is_empty() {
            return;
        }
        for user in &due {
            self.presence_seen.insert(user.clone(), now);
        }
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.presences(&due).await {
                Ok(presence) => out.send(AccountEvent::Presence(presence)),
                Err(error) => log::info!("presence unavailable: {error}"),
            }
        });
    }

    /// Fetches people whose presence Graph said changed. Unlike a look, a
    /// change notice is always worth one fetch.
    pub(super) fn presence_changed(&mut self, users: Vec<String>) {
        let Some(account) = &self.account else { return };
        if users.is_empty() || !account.graph.session().has_scope("Presence.Read.All") {
            return;
        }
        let now = Instant::now();
        for user in &users {
            self.presence_seen.insert(user.clone(), now);
        }
        log::debug!(
            "push[{}]: fetching presence for {} people",
            account_tag(&account.record.id),
            users.len()
        );
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.presences(&users).await {
                Ok(presence) => out.send(AccountEvent::Presence(presence)),
                Err(error) => log::info!("presence unavailable: {error}"),
            }
        });
    }
}
