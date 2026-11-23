//! App events: plugins.

use super::super::*;

impl App {
    pub(in crate::app) fn handle_relay_key_saved(&mut self, set: bool, ok: bool) {
        if !ok {
            self.toast(
                ToastKind::Error,
                "The access key could not be saved in the system's credential store.",
            );
            return;
        }
        self.settings.relay_key_set = set;
        self.mark_settings_dirty();
        if set {
            self.form_relay_key.clear();
        }
        // Reconnect every account with the new key.
        self.send_relay();
    }
}
