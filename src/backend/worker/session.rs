use super::*;

impl Worker {
    pub(super) async fn start(
        &mut self,
        client_id: String,
        tenant: String,
        account: Option<String>,
    ) {
        if !crate::settings::is_guid(client_id.trim()) {
            self.config = None;
            self.out.send(AccountEvent::Auth(AuthStatus::NotConfigured));
            return;
        }
        let config = AuthConfig {
            authority: self.options.authority.clone(),
            ..AuthConfig::new(&client_id, &tenant)
        };
        let changed = self.config.as_ref() != Some(&config);
        self.config = Some(config.clone());
        if !changed && let Some(account) = &self.account {
            // The window marks the account as restoring whenever it sends
            // Start (saving an unchanged registration does), so say again
            // that it is signed in.
            let record = &account.record;
            self.out.send(AccountEvent::Auth(AuthStatus::SignedIn {
                account: record.id.clone(),
                name: record.display_name.clone(),
                username: record.username.clone(),
            }));
            return;
        }
        self.account = None;
        self.store = None;
        self.expired = None;
        let Some(id) = account else {
            self.out.send(AccountEvent::Auth(AuthStatus::SignedOut));
            return;
        };
        let record_path = self.dirs.account(&id).join("account.json");
        let Some(record) = AccountRecord::load(&record_path) else {
            self.out.send(AccountEvent::Auth(AuthStatus::SignedOut));
            return;
        };
        self.out.send(AccountEvent::Auth(AuthStatus::Restoring));
        // The cache first, so the window has something to show at once.
        let store = match Store::open(&self.dirs.database(&id)) {
            Ok(store) => Arc::new(Mutex::new(store)),
            Err(error) => {
                log::warn!("could not open the cache: {error}");
                match Store::in_memory() {
                    Ok(store) => Arc::new(Mutex::new(store)),
                    Err(_) => return,
                }
            }
        };
        self.store = Some(Arc::clone(&store));
        emit_cache(&store, &self.out).await;
        self.resend_checked.lock().await.clear();
        let restored_from = Arc::clone(&store);
        match Session::restore(
            &record,
            config,
            self.http.clone(),
            Arc::clone(&self.secrets),
        )
        .await
        {
            Ok(Some(session)) => {
                let session = Arc::new(session);
                self.out.send(AccountEvent::Granted(session.granted()));
                self.out
                    .send(AccountEvent::ChannelAccess(channel_access(&session)));
                self.account = Some(Account {
                    graph: Arc::new(Graph::with_base(
                        self.http.clone(),
                        session,
                        &self.options.graph_base,
                    )),
                    record: record.clone(),
                    store,
                });
                self.out.send(AccountEvent::Auth(AuthStatus::SignedIn {
                    account: record.id.clone(),
                    name: record.display_name.clone(),
                    username: record.username.clone(),
                }));
                emit_restored(&restored_from, &self.out).await;
                self.restart_push().await;
            }
            Ok(None) => {
                self.out.send(AccountEvent::Auth(AuthStatus::Expired {
                    username: record.username.clone(),
                    reason: "Sign in again to get new messages.".into(),
                }));
                emit_restored(&restored_from, &self.out).await;
                self.expired = Some(record);
            }
            Err(error) => {
                self.out.send(AccountEvent::Auth(AuthStatus::Expired {
                    username: record.username.clone(),
                    reason: format!("Chaat could not read its saved session: {error}."),
                }));
                emit_restored(&restored_from, &self.out).await;
                self.expired = Some(record);
            }
        }
    }

    pub(super) fn sign_in(&mut self, extra_scopes: Vec<String>, device_code: bool) {
        let Some(config) = self.config.clone() else {
            self.out.send(AccountEvent::Auth(AuthStatus::NotConfigured));
            return;
        };
        if let Some(previous) = self.cancel_sign_in.take() {
            let _ = previous.send(true);
        }
        let (cancel, cancelled) = watch::channel(false);
        self.cancel_sign_in = Some(cancel);
        // Only scopes asked for now need a consent screen.
        let consent = !extra_scopes.is_empty();
        // Keep the scopes already granted when asking for more, and when
        // signing an expired account in again.
        let granted = match (&self.account, &self.expired) {
            (Some(account), _) => account.graph.session().granted(),
            (None, Some(record)) => record.granted.clone(),
            (None, None) => String::new(),
        };
        let mut extra = extra_scopes;
        for scope in auth::OPTIONAL_SCOPES {
            if auth::has_scope(&granted, scope) && !extra.iter().any(|s| s == scope) {
                extra.push((*scope).to_owned());
            }
        }
        let hint = self
            .account
            .as_ref()
            .map(|a| &a.record)
            .or(self.expired.as_ref())
            .map(|r| r.username.clone())
            .filter(|u| !u.is_empty());
        let http = self.http.clone();
        let secrets = Arc::clone(&self.secrets);
        let out = self.out.clone();
        let internal = self.internal.clone();
        let open_browser = self.options.open_browser;
        let sign_in_browser = self.sign_in_browser.clone();
        let stopped = cancelled.clone();
        tokio::spawn(async move {
            let result = if device_code {
                device_code_sign_in(&http, &config, &extra, &out, cancelled).await
            } else {
                browser_sign_in(
                    &http,
                    &config,
                    Ask {
                        extra: &extra,
                        hint: hint.as_deref(),
                        consent,
                    },
                    &out,
                    cancelled,
                    open_browser.then_some(sign_in_browser),
                )
                .await
            };
            let outcome = match result {
                Ok(response) => Session::from_sign_in(config, http, secrets, response)
                    .await
                    .map(|pair| Internal::SignedIn(Box::new(pair)))
                    .unwrap_or_else(|error| Internal::SignInFailed(error.to_string())),
                Err(reason) => Internal::SignInFailed(reason),
            };
            // A cancelled or replaced attempt reports nothing: the interface
            // has already moved on.
            if *stopped.borrow() {
                return;
            }
            let _ = internal.send(outcome);
        });
    }

    pub(super) async fn install(&mut self, session: Session, claims: IdClaims) {
        let id = session.account.clone();
        let account_dir = self.dirs.account(&id);
        let _ = crate::paths::create_private_dir(&account_dir);
        let record = AccountRecord {
            id: id.clone(),
            display_name: claims.name.clone().unwrap_or_default(),
            username: claims.preferred_username.clone().unwrap_or_default(),
            tenant_id: claims.tid.clone().unwrap_or_default(),
            granted: session.granted(),
            client_id: session.config.client_id.clone(),
            tenant: session.config.tenant.clone(),
        };
        if let Err(error) = record.save(&account_dir.join("account.json")) {
            log::warn!("could not save the account record: {error}");
        }
        let store = match self.account.take() {
            // Re-consent for more scopes keeps the same cache.
            Some(previous) if previous.record.id == id => previous.store,
            _ => match Store::open(&self.dirs.database(&id)).or_else(|_| Store::in_memory()) {
                Ok(store) => Arc::new(Mutex::new(store)),
                Err(error) => {
                    self.out.send(AccountEvent::Auth(AuthStatus::Failed(format!(
                        "Could not open the cache: {error}"
                    ))));
                    return;
                }
            },
        };
        let saved = session.token_saved();
        let store_locked = session.store_locked();
        let session = Arc::new(session);
        self.out.send(AccountEvent::Granted(session.granted()));
        self.out
            .send(AccountEvent::ChannelAccess(channel_access(&session)));
        self.account = Some(Account {
            graph: Arc::new(Graph::with_base(
                self.http.clone(),
                session,
                &self.options.graph_base,
            )),
            record: record.clone(),
            store: Arc::clone(&store),
        });
        self.store = Some(Arc::clone(&store));
        self.expired = None;
        self.freshness.clear();
        emit_cache(&store, &self.out).await;
        self.out.send(AccountEvent::Auth(AuthStatus::SignedIn {
            account: id,
            name: record.display_name,
            username: record.username,
        }));
        if !saved {
            self.out.send(AccountEvent::Error(
                unsaved_sign_in_text(store_locked).into(),
            ));
        }
        self.restart_push().await;
    }

    pub(super) async fn sign_out(&mut self) {
        self.stop_push().await;
        self.end_presence_session().await;
        self.store = None;
        let Some(account) = self.account.take() else {
            self.out.send(AccountEvent::Auth(AuthStatus::SignedOut));
            return;
        };
        account.graph.session().sign_out().await;
        // The cache holds the person's messages: remove it with the session.
        // Fetches still running hold the database open, and Windows will
        // not delete an open file, so wait for them to let go first.
        let store = account.store;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&store) > 1 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(store);
        let dir = self.dirs.account(&account.record.id);
        for attempt in 1..=10 {
            match tokio::fs::remove_dir_all(&dir).await {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) if attempt == 10 => {
                    log::warn!("could not remove the account's cache: {error}");
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        self.freshness.clear();
        self.out.send(AccountEvent::Auth(AuthStatus::SignedOut));
    }
}
