//! What a plugin can be for (its `ready.capabilities`) and what it may ask
//! Chaat to share (its `ready.needs`). Anything else a plugin names is
//! ignored.

/// Rings for incoming Teams calls. One such plugin at a time.
pub const CALL_ALERTS: &str = "call_alerts";
/// Shows the person's presence (a status light). Any number of plugins.
pub const PRESENCE_DISPLAY: &str = "presence_display";

/// The capabilities Chaat knows.
pub const KNOWN: &[&str] = &[CALL_ALERTS, PRESENCE_DISPLAY];

/// Needs a plugin may ask for: account ids and sign-in names, and the
/// person's availability and activity (never the status message).
const NEEDS: &[&str] = &["accounts", "presence"];

/// Whether only one enabled plugin may offer this capability.
pub fn is_exclusive(capability: &str) -> bool {
    capability == CALL_ALERTS
}

/// The known capabilities among `capabilities`, once each, in the order
/// Chaat lists them.
pub fn offered(capabilities: &[String]) -> Vec<&'static str> {
    KNOWN
        .iter()
        .copied()
        .filter(|known| capabilities.iter().any(|c| c == known))
        .collect()
}

/// The known needs among `raw`, once each; unknown needs are dropped.
pub fn needs(raw: &[String]) -> Vec<&'static str> {
    NEEDS
        .iter()
        .copied()
        .filter(|known| raw.iter().any(|n| n == known))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn offered_keeps_only_known_capabilities() {
        assert_eq!(
            offered(&strings(&["presence_display", "x"])),
            [PRESENCE_DISPLAY]
        );
        assert_eq!(
            offered(&strings(&[
                "presence_display",
                "call_alerts",
                "call_alerts"
            ])),
            [CALL_ALERTS, PRESENCE_DISPLAY]
        );
        assert!(offered(&[]).is_empty());
    }

    #[test]
    fn unknown_needs_are_dropped() {
        assert_eq!(
            needs(&strings(&["presence", "tokens", "accounts", "presence"])),
            ["accounts", "presence"]
        );
        assert!(needs(&strings(&["tokens", "messages"])).is_empty());
    }

    #[test]
    fn only_call_alerts_is_exclusive() {
        assert!(is_exclusive(CALL_ALERTS));
        assert!(!is_exclusive(PRESENCE_DISPLAY));
        assert!(!is_exclusive("other"));
    }
}
