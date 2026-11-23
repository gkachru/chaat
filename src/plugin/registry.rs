//! What a plugin can be for (its `ready.capabilities`) and what it may ask
//! Chaat to share (its `ready.needs`). Anything else a plugin names is
//! ignored.

/// Rings for incoming Teams calls. One such plugin at a time.
pub const CALL_ALERTS: &str = "call_alerts";
/// Shows the person's presence (a status light). Any number of plugins.
pub const PRESENCE_DISPLAY: &str = "presence_display";

/// Receives Graph change notifications through a public address and forwards
/// them to Chaat. One such plugin at a time.
pub const PUSH: &str = "push";

/// Lets an AI agent read cached messages (allowed chats only) and propose
/// replies and reactions the person approves. One such plugin at a time.
pub const AGENTS: &str = "agents";

/// The capabilities Chaat knows.
pub const KNOWN: &[&str] = &[CALL_ALERTS, PRESENCE_DISPLAY, PUSH, AGENTS];

/// Needs a plugin may ask for: account ids and sign-in names, and the
/// person's availability and activity (never the status message), and
/// per-account push channels, and cached messages for an agent.
const NEEDS: &[&str] = &["accounts", "presence", PUSH, "messages"];

/// A capability Chaat knows, the typed form of the strings in [`KNOWN`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    CallAlerts,
    PresenceDisplay,
    Push,
    Agents,
}

impl Capability {
    /// Every capability, in the order Chaat lists them.
    pub const ALL: [Capability; 4] = [
        Capability::CallAlerts,
        Capability::PresenceDisplay,
        Capability::Push,
        Capability::Agents,
    ];

    /// The name a plugin uses in `ready.capabilities`.
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::CallAlerts => CALL_ALERTS,
            Capability::PresenceDisplay => PRESENCE_DISPLAY,
            Capability::Push => PUSH,
            Capability::Agents => AGENTS,
        }
    }

    /// The capability named `name`, or `None` when Chaat does not know it.
    pub fn parse(name: &str) -> Option<Capability> {
        Self::ALL.into_iter().find(|c| c.as_str() == name)
    }

    /// Whether only one enabled plugin may offer this capability.
    pub fn is_exclusive(self) -> bool {
        matches!(
            self,
            Capability::CallAlerts | Capability::Push | Capability::Agents
        )
    }
}

/// A need Chaat knows, the typed form of the strings a plugin lists in
/// `ready.needs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Need {
    Accounts,
    Presence,
    Push,
    Messages,
}

impl Need {
    /// Every need, in the order Chaat lists them.
    pub const ALL: [Need; 4] = [Need::Accounts, Need::Presence, Need::Push, Need::Messages];

    /// The name a plugin uses in `ready.needs`.
    pub fn as_str(self) -> &'static str {
        match self {
            Need::Accounts => "accounts",
            Need::Presence => "presence",
            Need::Push => PUSH,
            Need::Messages => "messages",
        }
    }

    /// The need named `name`, or `None` when Chaat does not know it.
    pub fn parse(name: &str) -> Option<Need> {
        Self::ALL.into_iter().find(|n| n.as_str() == name)
    }
}

/// Whether only one enabled plugin may offer this capability.
pub fn is_exclusive(capability: &str) -> bool {
    Capability::parse(capability).is_some_and(Capability::is_exclusive)
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
        assert!(needs(&strings(&["tokens", "secrets"])).is_empty());
        assert_eq!(needs(&strings(&["messages", "x"])), ["messages"]);
    }

    #[test]
    fn registry_knows_push() {
        assert!(KNOWN.contains(&"push"));
        assert!(is_exclusive("push"));
        assert_eq!(needs(&strings(&["push", "x"])), ["push"]);
        assert_eq!(offered(&strings(&["push"])), [PUSH]);
    }

    #[test]
    fn registry_knows_agents() {
        assert!(KNOWN.contains(&"agents"));
        assert!(is_exclusive(AGENTS));
        assert_eq!(offered(&strings(&["agents", "x"])), [AGENTS]);
    }

    #[test]
    fn only_call_alerts_push_and_agents_are_exclusive() {
        assert!(is_exclusive(CALL_ALERTS));
        assert!(!is_exclusive(PRESENCE_DISPLAY));
        assert!(!is_exclusive("other"));
    }

    #[test]
    fn need_parse_round_trips() {
        for need in Need::ALL {
            assert_eq!(Need::parse(need.as_str()), Some(need));
        }
        assert_eq!(Need::parse("presence"), Some(Need::Presence));
        assert_eq!(Need::parse("status"), None);
        assert_eq!(Need::parse(""), None);
        let names: Vec<&str> = Need::ALL.iter().map(|n| n.as_str()).collect();
        assert_eq!(names, NEEDS);
    }

    #[test]
    fn capability_parse_round_trips() {
        for capability in Capability::ALL {
            assert_eq!(Capability::parse(capability.as_str()), Some(capability));
        }
        assert_eq!(Capability::Push.as_str(), "push");
        assert_eq!(Capability::parse("other"), None);
        let names: Vec<&str> = Capability::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(names, KNOWN);
    }

    #[test]
    fn only_call_alerts_push_agents_are_exclusive() {
        for capability in Capability::ALL {
            let expected = matches!(
                capability,
                Capability::CallAlerts | Capability::Push | Capability::Agents
            );
            assert_eq!(capability.is_exclusive(), expected);
            assert_eq!(is_exclusive(capability.as_str()), expected);
        }
        assert!(!Capability::PresenceDisplay.is_exclusive());
    }
}
