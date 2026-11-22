//! Agents (docs/plugins.md): one plugin offering
//! `agents` may read cached messages of the conversations the person allowed
//! and propose replies and reactions. Chaat answers reads from the cache,
//! shows every proposal for the person to decide, and never logs message
//! text.

use serde_json::json;

use super::App;
use crate::agents::access::{Access, AgentAccess};
use crate::agents::proposals::{Proposal, ProposalKind};
use crate::backend::Command;
use crate::model::{Action, ConversationId, Message, ToastKind};
use crate::plugin::agent::{AgentProposal, AgentQuery, result_line};
use crate::plugin::protocol::HostMessage;
use crate::plugin::registry;

/// The need that lets the agents plugin read and propose.
const MESSAGES: &str = "messages";

/// The most bytes of one event line, like an answer (`MAX_LINE` is 64 KiB).
const MAX_EVENT: usize = 60_000;

impl App {
    /// The plugin that gets agent answers and events: the first enabled one
    /// in list order that is running and offers `agents`.
    pub fn agents_owner(&self) -> Option<String> {
        self.settings
            .plugins
            .iter()
            .filter(|entry| entry.enabled)
            .find(|entry| {
                self.plugins
                    .get(&entry.id)
                    .is_some_and(|state| state.running && state.offers.contains(&registry::AGENTS))
            })
            .map(|entry| entry.id.clone())
    }

    /// The agents owner, when it declared `messages` and the person allowed
    /// it. `None` means nothing is read, answered or sent to any plugin.
    pub fn agents_active(&self) -> Option<String> {
        let owner = self.agents_owner()?;
        self.settings
            .plugins
            .iter()
            .find(|entry| entry.id == owner)
            .is_some_and(|entry| Self::plugin_need_effective(entry, MESSAGES))
            .then_some(owner)
    }

    fn agent_send(&self, plugin: &str, message: HostMessage) {
        self.send_service(Command::PluginSend {
            id: plugin.to_owned(),
            message,
        });
    }

    fn agent_answer(
        &self,
        plugin: &str,
        request: u64,
        data: Result<serde_json::Value, &'static str>,
    ) {
        self.agent_send(plugin, result_line(request, data));
    }

    fn agent_outcome(&self, plugin: &str, proposal: &str, outcome: &str) {
        self.agent_send(
            plugin,
            HostMessage::AgentOutcome {
                proposal: proposal.to_owned(),
                outcome: outcome.to_owned(),
            },
        );
    }

    fn account_index(&self, account: &str) -> Option<usize> {
        self.accounts
            .iter()
            .position(|a| a.id.as_deref() == Some(account))
    }

    /// What agents may use in an account: closed unless the person opened it.
    pub fn agent_access_of(&self, account: &str) -> AgentAccess {
        self.settings
            .agent_access
            .get(account)
            .cloned()
            .unwrap_or_default()
    }

    /// Makes the account the active one while `f` runs, the way events of a
    /// background account are applied.
    fn in_account<R>(&mut self, index: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let (active, hidden) = (self.active, self.hidden);
        self.active = index;
        self.hidden = index != active || hidden;
        let result = f(self);
        self.active = active;
        self.hidden = hidden;
        result
    }

    pub(super) fn on_agent_query(
        &mut self,
        plugin: &str,
        request: u64,
        account: String,
        query: AgentQuery,
    ) {
        if self.agents_active().as_deref() != Some(plugin) {
            return self.agent_answer(plugin, request, Err("not_allowed"));
        }
        let Some(index) = self.account_index(&account) else {
            return self.agent_answer(plugin, request, Err("no_account"));
        };
        let access = self.agent_access_of(&account);
        if !self.demo {
            self.accounts[index].backend.send(Command::AgentRead {
                plugin: plugin.to_owned(),
                request,
                query,
                access,
            });
        }
    }

    pub(super) fn on_agent_answer(
        &mut self,
        plugin: String,
        request: u64,
        data: Result<serde_json::Value, &'static str>,
    ) {
        // The host drops an answer for a plugin that is gone.
        self.agent_answer(&plugin, request, data);
    }

    pub(super) fn on_agent_propose(
        &mut self,
        plugin: &str,
        request: u64,
        account: String,
        proposal: AgentProposal,
    ) {
        if self.agents_active().as_deref() != Some(plugin) {
            return self.agent_answer(plugin, request, Err("not_allowed"));
        }
        let Some(index) = self.account_index(&account) else {
            return self.agent_answer(plugin, request, Err("no_account"));
        };
        let (key, agent) = match &proposal {
            AgentProposal::Reply {
                conversation,
                agent,
                ..
            }
            | AgentProposal::Reaction {
                conversation,
                agent,
                ..
            } => (conversation, agent.clone()),
        };
        let Some(conversation) = ConversationId::from_key(key) else {
            return self.agent_answer(plugin, request, Err("bad_request"));
        };
        if !self.agent_access_of(&account).can_propose(&conversation) {
            return self.agent_answer(plugin, request, Err("not_allowed"));
        }
        if let AgentProposal::Reaction { message, .. } = &proposal
            && !self.in_account(index, |app| app.message_loaded(&conversation, message))
        {
            return self.agent_answer(plugin, request, Err("not_found"));
        }
        let now = crate::time::now();
        let (reaction, added) = match proposal {
            AgentProposal::Reply { text, reply_to, .. } => (
                false,
                Ok(self.agent_proposals.reply(
                    plugin,
                    &account,
                    conversation.clone(),
                    text,
                    reply_to,
                    agent.clone(),
                    now,
                )),
            ),
            AgentProposal::Reaction { message, emoji, .. } => (
                true,
                self.agent_proposals.reaction(
                    plugin,
                    &account,
                    conversation.clone(),
                    message,
                    emoji,
                    agent.clone(),
                    now,
                ),
            ),
        };
        let (id, replaced) = match added {
            Ok(added) => added,
            Err(code) => return self.agent_answer(plugin, request, Err(code)),
        };
        if let Some(old) = replaced {
            self.agent_outcome(plugin, &old, "replaced");
        }
        let name = agent.replace(['\u{201c}', '\u{201d}'], "'");
        let what = if reaction { "reaction" } else { "reply" };
        self.in_account(index, |app| {
            let title = app.title_of(&conversation);
            app.toast(
                ToastKind::Info,
                format!("\u{201c}{name}\u{201d} proposes a {what} in {title}"),
            );
        });
        self.agent_answer(
            plugin,
            request,
            Ok(json!({"proposal_id": id, "state": "pending"})),
        );
    }

    /// Drops every proposal whose plugin is no longer the active agents
    /// plugin (removed, stopped, disabled, need revoked), whose account is
    /// gone, or whose conversation the person no longer lets agents use. A
    /// plugin that is still the owner hears `discarded`.
    pub(crate) fn prune_agent_proposals(&mut self) {
        let active = self.agents_active();
        let dropped = {
            let accounts: Vec<Option<String>> =
                self.accounts.iter().map(|a| a.id.clone()).collect();
            let access = &self.settings.agent_access;
            self.agent_proposals.retain(|p| {
                active.as_deref() == Some(p.plugin.as_str())
                    && accounts
                        .iter()
                        .any(|a| a.as_deref() == Some(p.account.as_str()))
                    && access
                        .get(&p.account)
                        .is_some_and(|a| a.can_propose(&p.conversation))
            })
        };
        for (id, plugin) in dropped {
            if active.as_deref() == Some(plugin.as_str()) {
                self.agent_outcome(&plugin, &id, "discarded");
            }
        }
    }

    /// The person wrote in a conversation of the active account: the pending
    /// reply no longer fits.
    pub(super) fn outdate_agent_proposals(&mut self, conversation: &ConversationId) {
        let Some(account) = self.id.clone() else {
            return;
        };
        for id in self.agent_proposals.outdate(&account, conversation) {
            if let Some(plugin) = self.agent_proposals.get(&id).map(|p| p.plugin.clone()) {
                self.agent_outcome(&plugin, &id, "outdated");
            }
        }
    }

    fn agent_event(&self, account: &str, event: serde_json::Value) {
        if let Some(plugin) = self.agents_active() {
            self.agent_send(
                &plugin,
                HostMessage::AgentEvent {
                    account: account.to_owned(),
                    event,
                },
            );
        }
    }

    fn agent_can_read(&self, conversation: &ConversationId) -> Option<String> {
        let account = self.id.clone()?;
        self.agent_access_of(&account)
            .can_read(conversation)
            .then_some(account)
    }

    /// The person's own message was sent in the active account.
    pub(super) fn agent_event_user_sent(&self, conversation: &ConversationId, message_id: &str) {
        if let Some(account) = self.agent_can_read(conversation) {
            self.agent_event(
                &account,
                json!({
                    "type": "user_sent",
                    "conversation_id": conversation.key(),
                    "message_id": message_id,
                }),
            );
        }
    }

    /// Genuinely new messages from others landed for the active account.
    pub(super) fn agent_events_new_messages(
        &self,
        conversation: &ConversationId,
        messages: &[Message],
    ) {
        if messages.is_empty() {
            return;
        }
        let Some(account) = self.agent_can_read(conversation) else {
            return;
        };
        for message in messages {
            if let Some(view) = crate::agents::view::message(message) {
                // One line must stay under the plugin line limit: cut an oversized text.
                let (mut fitted, _) = crate::agents::view::fit(vec![view], MAX_EVENT);
                let Some(view) = fitted.pop() else {
                    continue;
                };
                self.agent_event(
                    &account,
                    json!({
                        "type": "new_message",
                        "conversation_id": conversation.key(),
                        "message": view,
                    }),
                );
            }
        }
    }

    pub(super) fn apply_agent_action(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::SetAgentAccess { account, access } => {
                self.settings.agent_access.insert(account, access);
                self.mark_settings_dirty();
            }
            Action::LetAgentsRead { conversation, on } => {
                let Some(account) = self.id.clone() else {
                    return;
                };
                let key = conversation.key();
                let access = self.settings.agent_access.entry(account).or_default();
                let kind = match conversation {
                    ConversationId::Chat(_) => &mut access.chats,
                    ConversationId::Channel { .. } => &mut access.channels,
                };
                match (&mut *kind, on) {
                    (Access::None, true) => *kind = Access::Selected([key].into()),
                    (Access::Selected(keys), true) => {
                        keys.insert(key);
                    }
                    (Access::Selected(keys), false) => {
                        keys.remove(&key);
                    }
                    _ => {}
                }
                self.mark_settings_dirty();
            }
            Action::AgentSend { proposal } => {
                if let Some(p) = self.take_agent_proposal(&proposal, true)
                    && let ProposalKind::Reply { text, reply_to } = p.kind.clone()
                {
                    self.in_proposal_account(&p, |app| {
                        app.send(
                            p.conversation.clone(),
                            text,
                            reply_to,
                            Vec::new(),
                            Vec::new(),
                            Vec::new(),
                        );
                    });
                    self.agent_outcome(&p.plugin, &p.id, "sent");
                }
            }
            Action::AgentEdit { proposal } => {
                if let Some(p) = self.take_agent_proposal(&proposal, true)
                    && let ProposalKind::Reply { text, reply_to } = p.kind.clone()
                {
                    self.in_proposal_account(&p, |app| {
                        let draft = app.drafts.entry(p.conversation.clone()).or_default();
                        draft.text = text;
                        draft.reply_to = reply_to;
                        draft.editing = None;
                    });
                    self.agent_outcome(&p.plugin, &p.id, "edited");
                }
            }
            Action::AgentDiscard { proposal } => {
                if let Some(p) = self.agent_proposals.take(&proposal) {
                    self.agent_outcome(&p.plugin, &p.id, "discarded");
                }
            }
            Action::AgentAcceptReaction { proposal } => {
                if let Some(p) = self.take_agent_proposal(&proposal, false)
                    && let ProposalKind::Reaction { message, emoji } = p.kind.clone()
                {
                    let present = self
                        .account_index(&p.account)
                        .map(|index| {
                            self.in_account(index, |app| {
                                app.message_loaded(&p.conversation, &message)
                            })
                        })
                        .unwrap_or(false);
                    if !present {
                        self.agent_outcome(&p.plugin, &p.id, "outdated");
                        return;
                    }
                    self.in_proposal_account(&p, |app| {
                        app.apply_one(
                            Action::React {
                                conversation: p.conversation.clone(),
                                message,
                                emoji,
                                on: true,
                            },
                            ctx,
                        );
                    });
                    self.agent_outcome(&p.plugin, &p.id, "accepted");
                }
            }
            _ => {}
        }
    }

    /// The active account has this message loaded in `conversation`.
    fn message_loaded(&self, conversation: &ConversationId, message: &str) -> bool {
        self.conversations
            .get(conversation)
            .is_some_and(|c| c.messages.iter().any(|m| m.id == message))
    }

    /// Takes a pending proposal for a reply (`reply`) or a reaction. An
    /// outdated reply cannot be sent or edited as is: it stays for the
    /// person to discard.
    fn take_agent_proposal(&mut self, id: &str, reply: bool) -> Option<Proposal> {
        self.prune_agent_proposals();
        let p = self.agent_proposals.get(id)?;
        let right_kind = matches!(p.kind, ProposalKind::Reply { .. }) == reply;
        if !right_kind || (reply && p.outdated) {
            return None;
        }
        self.agent_proposals.take(id)
    }

    /// Runs `f` with the proposal's account active. The account may have
    /// signed out since: then nothing happens.
    fn in_proposal_account(&mut self, p: &Proposal, f: impl FnOnce(&mut Self)) {
        if let Some(index) = self.account_index(&p.account) {
            self.in_account(index, f);
        }
    }
}

#[cfg(test)]
mod tests;
