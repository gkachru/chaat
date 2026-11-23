//! What the person sees of agents (docs/plugins.md): the
//! proposed reply above the composer, the proposed reaction under a message,
//! and the "What agents can see" section of the agents plugin's page.
//!
//! Everything an agent wrote (its name, the text) is untrusted and shown as
//! plain text: never markdown or HTML. These views only push actions; the
//! app sends, edits, discards or accepts after the frame.

use egui::{Align, CornerRadius, Frame, Margin, Stroke, vec2};

use super::widgets::{self, PointerControls as _};
use super::widgets::{group, note};
use crate::agents::access::Access;
use crate::agents::proposals::{Proposal, ProposalKind};
use crate::app::App;
use crate::model::{Action, ConversationId};
use crate::plugin::registry;
use crate::theme::{self, Icon};

/// Longest agent name shown, in characters. The name is the plugin's word,
/// not Chaat's.
const NAME_CHARS: usize = 40;

fn name_of(proposal: &Proposal) -> String {
    crate::html::truncate(&proposal.agent, NAME_CHARS)
}

/// "just now", "5 min ago", "2 h ago", "3 d ago".
fn ago(created: i64, now: i64) -> String {
    let minutes = ((now - created) / 60_000).max(0);
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => format!("{} d ago", minutes / 1440),
    }
}

// -- Proposed reply -------------------------------------------------------------

/// The card above the composer of `conversation`, when an agent proposed a
/// reply there: Send, Edit and Discard.
pub fn proposal_card(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    actions: &mut Vec<Action>,
) {
    let Some(account) = app.id.as_deref() else {
        return;
    };
    let Some(proposal) = app.agent_proposals.reply_for(account, conversation) else {
        return;
    };
    let ProposalKind::Reply { text, reply_to } = &proposal.kind else {
        return;
    };
    let palette = app.palette;
    let name = name_of(proposal);
    let header = if proposal.outdated {
        format!("{name}\u{2019}s reply is outdated: you answered since")
    } else {
        format!(
            "{name} proposes a reply \u{B7} {}",
            ago(proposal.created, crate::time::now())
        )
    };
    let quoted = reply_to.as_deref().and_then(|id| {
        let message = app.conversation(conversation)?.find(id)?;
        let line = message.body.to_plain();
        let line = line.lines().find(|l| !l.trim().is_empty())?.trim();
        Some(format!(
            "{}: {}",
            message.author,
            crate::html::truncate(line, 120)
        ))
    });
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(
            1.0,
            if proposal.outdated {
                palette.warning
            } else {
                palette.accent
            },
        ))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.add(
                egui::Label::new(egui::RichText::new(header).font(theme::medium(13.0)).color(
                    if proposal.outdated {
                        palette.warning
                    } else {
                        palette.text
                    },
                ))
                .wrap()
                .selectable(false),
            );
            if let Some(quoted) = quoted {
                widgets::line(ui, &quoted, theme::regular(12.0), palette.dim);
            }
            egui::ScrollArea::vertical()
                .id_salt(("agent-proposal-text", &proposal.id))
                .max_height(140.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(theme::regular(13.0))
                                .color(palette.text),
                        )
                        .wrap(),
                    );
                });
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                if !proposal.outdated && widgets::primary_button(ui, &palette, "Send").clicked() {
                    actions.push(Action::AgentSend {
                        proposal: proposal.id.clone(),
                    });
                }
                if widgets::secondary_button(ui, &palette, "Edit").clicked() {
                    actions.push(Action::AgentEdit {
                        proposal: proposal.id.clone(),
                    });
                }
                if widgets::secondary_button(ui, &palette, "Discard").clicked() {
                    actions.push(Action::AgentDiscard {
                        proposal: proposal.id.clone(),
                    });
                }
            });
        });
    ui.add_space(6.0);
}

// -- Proposed reactions ------------------------------------------------------

/// The reactions agents proposed for one message.
pub fn pending_reactions<'a>(
    app: &'a App,
    conversation: &ConversationId,
    message: &str,
) -> Vec<&'a Proposal> {
    match app.id.as_deref() {
        Some(account) => app
            .agent_proposals
            .reactions_for(account, conversation, message),
        None => Vec::new(),
    }
}

/// One small pill per proposed reaction: `<Agent> <emoji>  Accept · Discard`.
pub fn reaction_pills(
    ui: &mut egui::Ui,
    app: &App,
    pills: &[&Proposal],
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        for proposal in pills {
            let ProposalKind::Reaction { emoji, .. } = &proposal.kind else {
                continue;
            };
            Frame::new()
                .fill(palette.window)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(13))
                .inner_margin(Margin::symmetric(10, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{} {emoji}", name_of(proposal)))
                                    .font(theme::regular(12.0))
                                    .color(palette.secondary),
                            )
                            .selectable(false),
                        );
                        ui.add_space(2.0);
                        let accept =
                            widgets::text_button(ui, palette, "Accept", theme::medium(12.0));
                        widgets::on_click(
                            &accept,
                            actions,
                            Action::AgentAcceptReaction {
                                proposal: proposal.id.clone(),
                            },
                        );
                        ui.label(
                            egui::RichText::new("\u{B7}")
                                .font(theme::regular(12.0))
                                .color(palette.dim),
                        );
                        let discard =
                            widgets::text_button(ui, palette, "Discard", theme::medium(12.0));
                        widgets::on_click(
                            &discard,
                            actions,
                            Action::AgentDiscard {
                                proposal: proposal.id.clone(),
                            },
                        );
                    });
                });
        }
    });
}

// -- Chat list and header menu -----------------------------------------------

/// Whether the chat list marks `conversation`: something is pending in it.
pub fn has_pending(app: &App, conversation: &ConversationId) -> bool {
    app.id
        .as_deref()
        .is_some_and(|account| app.agent_proposals.has_pending(account, conversation))
}

/// What the header menu offers for a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadMenu {
    /// No agents plugin is on: no entry.
    Hidden,
    /// "Let agents read this chat".
    Let,
    /// "Stop agents reading this chat".
    Stop,
    /// This kind is open to all: nothing to add or remove here.
    Everything,
}

impl ReadMenu {
    pub fn of(app: &App, conversation: &ConversationId) -> Self {
        let (Some(_), Some(account)) = (app.agents_owner(), app.id.as_deref()) else {
            return Self::Hidden;
        };
        let access = app.agent_access_of(account);
        let kind = match conversation {
            ConversationId::Chat(_) => &access.chats,
            ConversationId::Channel { .. } => &access.channels,
        };
        match kind {
            Access::All => Self::Everything,
            Access::Selected(keys) if keys.contains(&conversation.key()) => Self::Stop,
            _ => Self::Let,
        }
    }
}

/// The popup id of a conversation's header menu.
pub fn menu_id(conversation: &ConversationId) -> egui::Id {
    egui::Id::new(("agents-chat-menu", conversation.key()))
}

/// The wording of the header menu entry, or `None` when there is none.
pub fn menu_label(menu: ReadMenu, conversation: &ConversationId) -> Option<&'static str> {
    let channel = conversation.is_channel();
    match (menu, channel) {
        (ReadMenu::Let, false) => Some("Let agents read this chat"),
        (ReadMenu::Let, true) => Some("Let agents read this channel"),
        (ReadMenu::Stop, false) => Some("Stop agents reading this chat"),
        (ReadMenu::Stop, true) => Some("Stop agents reading this channel"),
        _ => None,
    }
}

// -- Settings: what agents can see -------------------------------------------

/// "What agents can see" on the page of a plugin that offers `agents`: per
/// account, which chats and channels agents may read, and whether they may
/// propose. Draws nothing for any other plugin.
pub fn access_section(ui: &mut egui::Ui, app: &App, plugin: &str, actions: &mut Vec<Action>) {
    let offers = app
        .plugins
        .get(plugin)
        .is_some_and(|state| state.offers.contains(&registry::AGENTS));
    if !offers {
        return;
    }
    let palette = app.palette;
    group(ui, &palette, "What agents can see");
    note(
        ui,
        &palette,
        "Everything starts closed. Agents read only what you allow here, and \
         nothing they propose is sent without your click.",
    );
    let accounts = app.plugin_accounts();
    if accounts.is_empty() {
        ui.add_space(4.0);
        note(ui, &palette, "Sign in to choose what agents can see.");
        return;
    }
    for account in accounts {
        let access = app.agent_access_of(&account.id);
        let mut next = access.clone();
        ui.add_space(10.0);
        let name = if account.username.is_empty() {
            account.id.clone()
        } else {
            account.username.clone()
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .font(theme::medium(13.0))
                    .color(palette.text),
            )
            .truncate(),
        );
        ui.add_space(2.0);
        let owner = app
            .accounts
            .iter()
            .find(|a| a.id.as_deref() == Some(account.id.as_str()));
        kind_row(
            ui,
            app,
            owner,
            "Chats",
            &mut next.chats,
            "No chats chosen yet.",
        );
        kind_row(
            ui,
            app,
            owner,
            "Channels",
            &mut next.channels,
            "No channels chosen yet.",
        );
        if matches!(next.chats, Access::Selected(_)) || matches!(next.channels, Access::Selected(_))
        {
            note(
                ui,
                &palette,
                "To add one, open it and choose Let agents read this chat in its menu.",
            );
        }
        ui.add_space(4.0);
        ui.checkbox_hand(
            &mut next.propose,
            "Agents may propose replies and reactions",
        );
        if next != access {
            actions.push(Action::SetAgentAccess {
                account: account.id.clone(),
                access: next,
            });
        }
        ui.add_space(6.0);
        ui.separator();
    }
}

/// One kind (chats or channels): None, Selected or All, and under Selected
/// the chosen conversations with a remove button each.
fn kind_row(
    ui: &mut egui::Ui,
    app: &App,
    owner: Option<&crate::app::Account>,
    label: &str,
    access: &mut Access,
    empty: &str,
) {
    let palette = app.palette;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.allocate_ui_with_layout(
            vec2(68.0, 28.0),
            egui::Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .font(theme::regular(13.0))
                        .color(palette.secondary),
                );
            },
        );
        let (none, selected, all) = (
            matches!(access, Access::None),
            matches!(access, Access::Selected(_)),
            matches!(access, Access::All),
        );
        if widgets::chip(ui, &palette, "None", none).clicked() && !none {
            *access = Access::None;
        }
        if widgets::chip(ui, &palette, "Selected", selected).clicked() && !selected {
            *access = Access::Selected(Default::default());
        }
        if widgets::chip(ui, &palette, "All", all).clicked() && !all {
            *access = Access::All;
        }
    });
    if let Access::Selected(keys) = access {
        let mut remove = None;
        ui.indent(("agent-selected", label), |ui| {
            if keys.is_empty() {
                note(ui, &palette, empty);
            }
            for key in keys.iter() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let title = ConversationId::from_key(key)
                        .and_then(|c| owner.map(|o| o.title_of(&c)))
                        .unwrap_or_else(|| "Unknown conversation".into());
                    let close = widgets::icon_button_sized(
                        ui,
                        &palette,
                        Icon::Close,
                        "Stop agents reading this",
                        22.0,
                        14.0,
                        palette.dim,
                    );
                    if close.clicked() {
                        remove = Some(key.clone());
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title)
                                .font(theme::regular(13.0))
                                .color(palette.text),
                        )
                        .truncate(),
                    );
                });
            }
        });
        if let Some(key) = remove {
            keys.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn ago_reads_in_minutes_hours_and_days() {
        let min = 60_000;
        assert_eq!(ago(0, 30_000), "just now");
        assert_eq!(ago(0, 2 * min), "2 min ago");
        assert_eq!(ago(0, 59 * min), "59 min ago");
        assert_eq!(ago(0, 60 * min), "1 h ago");
        assert_eq!(ago(0, 24 * 60 * min), "1 d ago");
        assert_eq!(ago(10, 0), "just now");
    }
}
