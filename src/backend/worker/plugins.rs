use super::*;

impl Worker {
    /// Launches (or relaunches) one plugin of the plugin list. Only that id's
    /// host is replaced. An id whose previous run is still stopping starts
    /// when that run has ended.
    pub(super) fn start_plugin(&mut self, id: String, start: PluginStart) {
        if self.stopping_ids.values().any(|stopping| *stopping == id) {
            self.deferred_starts.insert(id, start);
            return;
        }
        self.launch_plugin(id, start);
    }

    pub(super) fn launch_plugin(&mut self, id: String, start: PluginStart) {
        let out = self.out.clone();
        let tag = id.clone();
        let generation = start.generation;
        let plugin = crate::plugin::host::Plugin::start(
            start.path,
            start.hello,
            crate::plugin::registry::KNOWN,
            start.expected_sha256,
            move |reported| {
                if let Some(event) = crate::plugin::event::event(reported) {
                    out.send(ServiceEvent::Plugin {
                        id: tag.clone(),
                        generation,
                        event,
                    });
                }
            },
        );
        if let Some(old) = self.plugins.insert(id, plugin) {
            old.abort();
        }
    }

    /// Stops one plugin without holding up the command loop: the stop runs in
    /// a tracked task, which shutdown waits for.
    pub(super) fn stop_plugin(&mut self, id: String) {
        self.deferred_starts.remove(&id);
        if let Some(plugin) = self.plugins.remove(&id) {
            let task = self.stopping.spawn(plugin.stop());
            self.stopping_ids.insert(task.id(), id);
        }
        self.prune_secret_queues();
    }

    /// A stop task ended: starts what was waiting for it.
    pub(super) fn plugin_stopped(
        &mut self,
        stopped: Result<(tokio::task::Id, ()), tokio::task::JoinError>,
    ) {
        let task = match stopped {
            Ok((task, ())) => task,
            Err(error) => {
                log::warn!("plugin: a stop task failed");
                error.id()
            }
        };
        let Some(id) = self.stopping_ids.remove(&task) else {
            return;
        };
        if !self.stopping_ids.values().any(|stopping| *stopping == id)
            && let Some(start) = self.deferred_starts.remove(&id)
        {
            self.launch_plugin(id, start);
        }
        self.prune_secret_queues();
    }

    /// One plugin's credential store queue, started on first use.
    pub(super) fn secret_queue(&mut self, plugin: &str) -> &SecretQueue {
        self.prune_secret_queues();
        self.secret_jobs
            .entry(plugin.to_owned())
            .or_insert_with(SecretQueue::start)
    }

    /// Drops the queues of plugins that are not running and have nothing
    /// queued. A queue with work left stays, so a later job for the same
    /// plugin still runs after it.
    pub(super) fn prune_secret_queues(&mut self) {
        let plugins = &self.plugins;
        self.secret_jobs
            .retain(|id, queue| !queue.idle() || plugins.contains_key(id));
    }

    /// Sends a plugin its options, with the stored secret values merged in.
    /// Values are never logged.
    pub(super) fn plugin_options(
        &mut self,
        id: String,
        mut values: std::collections::BTreeMap<String, serde_json::Value>,
        secrets: Vec<String>,
    ) {
        let Some(plugin) = self.plugins.get(&id) else {
            return;
        };
        let store = Arc::clone(&self.secrets);
        let outgoing = plugin.sender();
        self.secret_queue(&id).run(move || {
            for option in secrets {
                if let Ok(Some(value)) = store.read(&plugin_secret_key(&id, &option)) {
                    values.insert(option, serde_json::Value::String(value));
                }
            }
            let _ = outgoing.send(crate::plugin::protocol::HostMessage::Options { values });
        });
    }

    /// Stops every plugin of the plugin list, all at once, and waits for
    /// every stop, including ones already under way.
    pub(super) async fn stop_plugins(&mut self) {
        self.deferred_starts.clear();
        for (id, plugin) in self.plugins.drain() {
            let task = self.stopping.spawn(plugin.stop());
            self.stopping_ids.insert(task.id(), id);
        }
        let stops = &mut self.stopping;
        let all = async { while stops.join_next().await.is_some() {} };
        if tokio::time::timeout(PLUGIN_STOP_WAIT, all).await.is_err() {
            log::warn!("plugin: stopping took too long; giving up on the rest");
            self.stopping.abort_all();
        }
        self.stopping_ids.clear();
    }
}
