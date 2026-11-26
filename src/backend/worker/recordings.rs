use super::*;

impl Worker {
    /// Answers `AccountCommand::Recordings` from the local cache only.
    pub(super) fn recordings(&mut self) {
        let Some(account) = &self.account else { return };
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        tokio::spawn(async move {
            let list = {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                store.recordings(200).unwrap_or_else(|_| {
                    log::warn!("could not read the cached recordings");
                    Vec::new()
                })
            };
            out.send(AccountEvent::Recordings(list));
        });
    }
}
