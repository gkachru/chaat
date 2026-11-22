//! Pending reply and reaction proposals from agents (pure state).
//!
//! A proposal is never sent by itself: the interface shows it and the person
//! sends, edits, discards or accepts it. Proposals live in memory only.

use crate::model::{ConversationId, Millis};

/// Most pending reactions per conversation and account.
pub const MAX_REACTIONS: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposalKind {
    Reply {
        text: String,
        reply_to: Option<String>,
    },
    Reaction {
        message: String,
        emoji: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// `p1`, `p2`, ...: never reused within a run.
    pub id: String,
    /// The plugin that made it.
    pub plugin: String,
    /// The account (`oid.tid`) the conversation belongs to.
    pub account: String,
    pub conversation: ConversationId,
    pub kind: ProposalKind,
    /// The agent's own name, as the plugin reported it. Untrusted text.
    pub agent: String,
    pub created: Millis,
    /// The person wrote in the conversation since: the card stays but can no
    /// longer be sent as is.
    pub outdated: bool,
}

#[derive(Debug, Default)]
pub struct Proposals {
    next: u64,
    items: Vec<Proposal>,
}

impl Proposals {
    fn id(&mut self) -> String {
        self.next += 1;
        format!("p{}", self.next)
    }

    fn remove_where(&mut self, keep: impl Fn(&Proposal) -> bool) -> Option<String> {
        let at = self.items.iter().position(|p| !keep(p))?;
        Some(self.items.remove(at).id)
    }

    /// Adds a reply, replacing the pending one of that conversation. Returns
    /// the new id and the replaced id.
    #[allow(clippy::too_many_arguments)]
    pub fn reply(
        &mut self,
        plugin: &str,
        account: &str,
        conversation: ConversationId,
        text: String,
        reply_to: Option<String>,
        agent: String,
        now: Millis,
    ) -> (String, Option<String>) {
        let superseded = self.remove_where(|p| {
            !(p.account == account
                && p.conversation == conversation
                && matches!(p.kind, ProposalKind::Reply { .. }))
        });
        let id = self.id();
        self.items.push(Proposal {
            id: id.clone(),
            plugin: plugin.to_owned(),
            account: account.to_owned(),
            conversation,
            kind: ProposalKind::Reply { text, reply_to },
            agent,
            created: now,
            outdated: false,
        });
        (id, superseded)
    }

    /// Adds a reaction, replacing a pending one on the same message. Returns
    /// the new id and the replaced id, or `Err("busy")` when the conversation
    /// already holds [`MAX_REACTIONS`] others.
    #[allow(clippy::too_many_arguments)]
    pub fn reaction(
        &mut self,
        plugin: &str,
        account: &str,
        conversation: ConversationId,
        message: String,
        emoji: String,
        agent: String,
        now: Millis,
    ) -> Result<(String, Option<String>), &'static str> {
        let same_message = |p: &Proposal| {
            p.account == account
                && p.conversation == conversation
                && matches!(&p.kind, ProposalKind::Reaction { message: m, .. } if *m == message)
        };
        if !self.items.iter().any(same_message) {
            let pending = self
                .items
                .iter()
                .filter(|p| {
                    p.account == account
                        && p.conversation == conversation
                        && matches!(p.kind, ProposalKind::Reaction { .. })
                })
                .count();
            if pending >= MAX_REACTIONS {
                return Err("busy");
            }
        }
        let superseded = self.remove_where(|p| !same_message(p));
        let id = self.id();
        self.items.push(Proposal {
            id: id.clone(),
            plugin: plugin.to_owned(),
            account: account.to_owned(),
            conversation,
            kind: ProposalKind::Reaction { message, emoji },
            agent,
            created: now,
            outdated: false,
        });
        Ok((id, superseded))
    }

    /// Removes the proposals `keep` rejects. Returns their ids with their plugin.
    pub fn retain(&mut self, keep: impl Fn(&Proposal) -> bool) -> Vec<(String, String)> {
        let mut dropped = Vec::new();
        self.items.retain(|p| {
            let stays = keep(p);
            if !stays {
                dropped.push((p.id.clone(), p.plugin.clone()));
            }
            stays
        });
        dropped
    }

    /// Removes and returns a proposal.
    pub fn take(&mut self, id: &str) -> Option<Proposal> {
        let at = self.items.iter().position(|p| p.id == id)?;
        Some(self.items.remove(at))
    }

    pub fn get(&self, id: &str) -> Option<&Proposal> {
        self.items.iter().find(|p| p.id == id)
    }

    /// Marks the pending reply of a conversation outdated (it stays). Returns
    /// the ids that became outdated now.
    pub fn outdate(&mut self, account: &str, conversation: &ConversationId) -> Vec<String> {
        let mut ids = Vec::new();
        for p in &mut self.items {
            if p.account == account
                && p.conversation == *conversation
                && matches!(p.kind, ProposalKind::Reply { .. })
                && !p.outdated
            {
                p.outdated = true;
                ids.push(p.id.clone());
            }
        }
        ids
    }

    pub fn reply_for(&self, account: &str, conversation: &ConversationId) -> Option<&Proposal> {
        self.items.iter().find(|p| {
            p.account == account
                && p.conversation == *conversation
                && matches!(p.kind, ProposalKind::Reply { .. })
        })
    }

    pub fn reactions_for(
        &self,
        account: &str,
        conversation: &ConversationId,
        message: &str,
    ) -> Vec<&Proposal> {
        self.items
            .iter()
            .filter(|p| {
                p.account == account
                    && p.conversation == *conversation
                    && matches!(&p.kind, ProposalKind::Reaction { message: m, .. } if m == message)
            })
            .collect()
    }

    /// Whether anything is pending in the conversation, for list markers.
    pub fn has_pending(&self, account: &str, conversation: &ConversationId) -> bool {
        self.items
            .iter()
            .any(|p| p.account == account && p.conversation == *conversation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str) -> ConversationId {
        ConversationId::Chat(id.into())
    }

    fn reply(
        p: &mut Proposals,
        account: &str,
        conversation: &str,
        text: &str,
    ) -> (String, Option<String>) {
        p.reply(
            "agents",
            account,
            chat(conversation),
            text.into(),
            None,
            "Claude Code".into(),
            1,
        )
    }

    fn react(
        p: &mut Proposals,
        account: &str,
        conversation: &str,
        message: &str,
    ) -> Result<(String, Option<String>), &'static str> {
        p.reaction(
            "agents",
            account,
            chat(conversation),
            message.into(),
            "👍".into(),
            "A".into(),
            1,
        )
    }

    #[test]
    fn examples() {
        let mut p = Proposals::default();
        assert_eq!(reply(&mut p, "acc", "a", "one"), ("p1".into(), None));
        assert_eq!(
            reply(&mut p, "acc", "a", "two"),
            ("p2".into(), Some("p1".into()))
        );
        assert!(p.get("p1").is_none());
        assert_eq!(p.reply_for("acc", &chat("a")).unwrap().id, "p2");
        assert!(p.has_pending("acc", &chat("a")));
        assert!(!p.has_pending("acc", &chat("b")));
        assert_eq!(p.outdate("acc", &chat("a")), ["p2"]);
        assert!(p.reply_for("acc", &chat("a")).unwrap().outdated);
        assert!(p.outdate("acc", &chat("a")).is_empty());
        // A new reply replaces an outdated one.
        assert_eq!(
            reply(&mut p, "acc", "a", "three"),
            ("p3".into(), Some("p2".into()))
        );
        assert!(!p.reply_for("acc", &chat("a")).unwrap().outdated);
        let taken = p.take("p3").unwrap();
        assert!(matches!(taken.kind, ProposalKind::Reply { ref text, .. } if text == "three"));
        assert!(p.take("p3").is_none());
    }

    #[test]
    fn accounts_do_not_mix() {
        let mut p = Proposals::default();
        assert_eq!(reply(&mut p, "one", "a", "x").1, None);
        assert_eq!(reply(&mut p, "two", "a", "y").1, None);
        assert_eq!(p.reply_for("one", &chat("a")).unwrap().account, "one");
        assert_eq!(p.reply_for("two", &chat("a")).unwrap().account, "two");
        assert_eq!(p.outdate("one", &chat("a")), ["p1"]);
        assert!(!p.reply_for("two", &chat("a")).unwrap().outdated);
        assert_eq!(react(&mut p, "one", "a", "m").unwrap().1, None);
        assert_eq!(react(&mut p, "two", "a", "m").unwrap().1, None);
    }

    #[test]
    fn reaction_supersedes_on_same_message() {
        let mut p = Proposals::default();
        let (first, _) = react(&mut p, "acc", "a", "m1").unwrap();
        react(&mut p, "acc", "a", "m2").unwrap();
        let (_, replaced) = react(&mut p, "acc", "a", "m1").unwrap();
        assert_eq!(replaced, Some(first));
        assert_eq!(p.reactions_for("acc", &chat("a"), "m1").len(), 1);
        assert_eq!(p.reactions_for("acc", &chat("a"), "m2").len(), 1);
        // A reply and a reaction are independent.
        assert_eq!(reply(&mut p, "acc", "a", "x").1, None);
        assert_eq!(p.outdate("acc", &chat("a")).len(), 1);
    }

    #[test]
    fn reaction_cap() {
        let mut p = Proposals::default();
        for i in 0..MAX_REACTIONS {
            react(&mut p, "acc", "a", &format!("m{i}")).unwrap();
        }
        assert_eq!(react(&mut p, "acc", "a", "extra"), Err("busy"));
        // Replacing one on an existing message still works at the cap.
        assert!(react(&mut p, "acc", "a", "m0").unwrap().1.is_some());
        // Other conversations and accounts have their own cap.
        assert!(react(&mut p, "acc", "b", "extra").is_ok());
        assert!(react(&mut p, "other", "a", "extra").is_ok());
        // Taking one frees a place.
        let id = p.reactions_for("acc", &chat("a"), "m1")[0].id.clone();
        p.take(&id).unwrap();
        assert!(react(&mut p, "acc", "a", "extra").is_ok());
    }

    #[test]
    fn ids_unique_after_take() {
        let mut p = Proposals::default();
        let (a, _) = reply(&mut p, "acc", "a", "x");
        p.take(&a).unwrap();
        let (b, _) = reply(&mut p, "acc", "a", "y");
        assert_ne!(a, b);
        assert_eq!(b, "p2");
    }
}
