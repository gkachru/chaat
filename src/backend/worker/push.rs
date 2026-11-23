use super::*;

impl Worker {
    /// Stops any running push loop and, when a relay is set and an account is
    /// signed in, starts a new one.
    pub(super) async fn restart_push(&mut self) {
        self.stop_push().await;
        let Some(account) = &self.account else {
            self.out.send(AccountEvent::Push(PushStatus::Off));
            return;
        };
        // A relay set by hand wins over a push plugin's route.
        let (mut source, bodies) = if let Some(relay) = self.relay.clone() {
            (PushSource::Relay { relay, key: None }, None)
        } else if let Some(route) = self.push_route.clone() {
            let (tx, rx) = mpsc::channel(PUSH_BODY_QUEUE);
            (PushSource::Plugin { route, bodies: rx }, Some(tx))
        } else {
            self.out.send(AccountEvent::Push(PushStatus::Off));
            return;
        };
        let (stop, stopped) = watch::channel(false);
        let graph = Arc::clone(&account.graph);
        let me = account.record.user_id().to_owned();
        let out = self.out.clone();
        let internal = self.internal.clone();
        let secrets = Arc::clone(&self.secrets);
        if let PushSource::Relay { key, .. } = &mut source {
            *key = tokio::task::spawn_blocking(move || secrets.read(crate::secrets::RELAY_KEY))
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
        }
        let target = PushTarget {
            me,
            source,
            leftovers: self
                .dirs
                .account(&account.record.id)
                .join(realtime::LEFTOVERS_FILE),
            contacts: self.contacts.subscribe(),
            refused: self
                .dirs
                .account(&account.record.id)
                .join(realtime::REFUSED_FILE),
            renew_every: self.options.push_renew_every,
            connect_within: self.options.relay_connect_timeout,
        };
        let tag = account_tag(&account.record.id);
        let task =
            tokio::spawn(PUSH_ACCOUNT.scope(tag, push_loop(graph, target, out, internal, stopped)));
        self.stop_push = Some((stop, task));
        self.push_bodies = bodies;
        self.push_dropped = false;
    }

    /// Stops the push loop and waits for it to delete its subscriptions,
    /// so they don't keep sending notifications to a closed channel (and a
    /// new presence subscription isn't refused as a duplicate). A slow or
    /// unreachable Graph cannot hold up quitting for longer than
    /// [`PUSH_CLEANUP`]; whatever is left is deleted at the next start.
    pub(super) async fn stop_push(&mut self) {
        if let Some((stop, task)) = self.stop_push.take() {
            // Stop first: closing the body queue before the stop flag is set
            // would let the loop report "the push plugin stopped" as a failure.
            let _ = stop.send(true);
            self.push_bodies = None;
            if tokio::time::timeout(PUSH_CLEANUP, task).await.is_err() {
                log::info!(
                    "push[{}]: cleanup took too long; the next start finishes it",
                    self.me_id().as_deref().map(account_tag).unwrap_or_default()
                );
            }
        }
    }
}
