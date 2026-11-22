//! The per-account allow list for agents. Everything starts closed.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::model::ConversationId;

/// Which conversations of one kind agents may use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    /// No conversation of this kind.
    #[default]
    None,
    /// Only these, by `ConversationId::key()`.
    Selected(BTreeSet<String>),
    /// Every conversation of this kind.
    All,
}

impl Access {
    fn allows(&self, key: &str) -> bool {
        match self {
            Self::None => false,
            Self::Selected(keys) => keys.contains(key),
            Self::All => true,
        }
    }
}

/// What agents may do in one signed-in account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentAccess {
    pub chats: Access,
    pub channels: Access,
    /// Agents may propose replies and reactions in readable conversations.
    /// A proposal is only ever shown for the person to send.
    pub propose: bool,
}

impl AgentAccess {
    pub fn can_read(&self, conversation: &ConversationId) -> bool {
        let access = match conversation {
            ConversationId::Chat(_) => &self.chats,
            ConversationId::Channel { .. } => &self.channels,
        };
        access.allows(&conversation.key())
    }

    /// Proposing needs read access too: an agent cannot answer what it may
    /// not see.
    pub fn can_propose(&self, conversation: &ConversationId) -> bool {
        self.propose && self.can_read(conversation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str) -> ConversationId {
        ConversationId::Chat(id.into())
    }

    #[test]
    fn examples() {
        let open = AgentAccess {
            chats: Access::All,
            channels: Access::None,
            propose: false,
        };
        assert!(open.can_read(&chat("a")));
        assert!(!open.can_propose(&chat("a")));
        assert!(!AgentAccess::default().can_read(&chat("a")));
        let both = AgentAccess {
            propose: true,
            ..open.clone()
        };
        assert!(both.can_propose(&chat("a")));
        let channel = ConversationId::Channel {
            team: "t".into(),
            channel: "c".into(),
        };
        assert!(!both.can_read(&channel));
        assert!(!both.can_propose(&channel));
    }

    #[test]
    fn selected_matches_key() {
        let access = AgentAccess {
            channels: Access::Selected(["channel:t:c".to_owned()].into()),
            chats: Access::Selected(["channel:t:c".to_owned()].into()),
            propose: true,
        };
        let yes = ConversationId::Channel {
            team: "t".into(),
            channel: "c".into(),
        };
        let other = ConversationId::Channel {
            team: "t".into(),
            channel: "d".into(),
        };
        assert!(access.can_read(&yes));
        assert!(!access.can_read(&other));
        // A chat is checked against the chats list, not the channels one.
        assert!(!access.can_read(&chat("channel:t:c")));
    }

    #[test]
    fn old_settings_load() {
        let settings: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.agent_access.is_empty());
        let access: AgentAccess = serde_json::from_str("{}").unwrap();
        assert_eq!(access, AgentAccess::default());
    }

    #[test]
    fn round_trips() {
        let access = AgentAccess {
            chats: Access::Selected(["chat:x".to_owned()].into()),
            channels: Access::All,
            propose: true,
        };
        let json = serde_json::to_string(&access).unwrap();
        assert_eq!(serde_json::from_str::<AgentAccess>(&json).unwrap(), access);
    }
}
