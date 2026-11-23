//! Channel post cards with their replies.

use super::message::{body, message_row, reactions};
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn thread_card(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    root: &Message,
    me: &str,
    now: i64,
    expanded: bool,
    actions: &mut Vec<Action>,
) -> egui::Rect {
    let palette = &app.palette;
    let width = ui.available_width();
    if root.system {
        let width = ui.available_width();
        return message_row(
            ui,
            app,
            conversation,
            root,
            me,
            false,
            false,
            width,
            actions,
        )
        .0;
    }
    ui.add_space(6.0);
    let response = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(
            1.0,
            if root.importance_high {
                palette.danger.gamma_multiply(0.6)
            } else {
                palette.outline
            },
        ))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin {
            left: 12,
            right: 14,
            top: 10,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(width - 26.0);
            let inner_width = width - 26.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                widgets::avatar(ui, app, root.author_id.as_deref(), &root.author, 34.0, true);
                ui.vertical(|ui| {
                    let content = inner_width - 44.0;
                    ui.set_max_width(content);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&root.author)
                                .font(theme::semibold(14.0))
                                .color(palette.text),
                        );
                        ui.label(
                            egui::RichText::new(stamp(root.created, now))
                                .font(theme::regular(12.0))
                                .color(palette.dim),
                        );
                        if root.importance_high {
                            ui.label(
                                egui::RichText::new("IMPORTANT")
                                    .font(theme::bold(11.0))
                                    .color(palette.danger),
                            );
                        }
                    });
                    if let Some(subject) = &root.subject {
                        ui.label(
                            egui::RichText::new(subject)
                                .font(theme::bold(15.0))
                                .color(palette.text),
                        );
                    }
                    body(ui, app, conversation, root, content, actions);
                    if !root.reactions.is_empty() {
                        reactions(ui, app, conversation, root, me, actions);
                    }
                });
            });
            // Replies.
            ui.add_space(6.0);
            let count = root.replies.len();
            if count > 0 {
                let line = ui.cursor().top();
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    line,
                    Stroke::new(1.0, palette.outline),
                );
                ui.add_space(6.0);
                if !expanded && count > 2 {
                    let mut names: Vec<&str> =
                        root.replies.iter().map(|r| r.author.as_str()).collect();
                    names.sort_unstable();
                    names.dedup();
                    let text = format!(
                        "{count} replies from {}",
                        names.into_iter().take(3).collect::<Vec<_>>().join(", ")
                    );
                    if ui
                        .link(
                            egui::RichText::new(text)
                                .font(theme::semibold(12.0))
                                .color(palette.accent),
                        )
                        .clicked()
                    {
                        actions.push(Action::ToggleThread(root.id.clone()));
                        // Expanding shows the thread as Graph has it now.
                        actions.push(Action::LoadReplies {
                            conversation: conversation.clone(),
                            message: root.id.clone(),
                        });
                    }
                }
                let shown: Vec<&Message> = if expanded || count <= 2 {
                    root.replies.iter().collect()
                } else {
                    root.replies.iter().rev().take(1).collect()
                };
                for reply in shown {
                    ui.horizontal_top(|ui| {
                        ui.add_space(8.0);
                        ui.spacing_mut().item_spacing.x = 8.0;
                        widgets::avatar(
                            ui,
                            app,
                            reply.author_id.as_deref(),
                            &reply.author,
                            26.0,
                            true,
                        );
                        ui.vertical(|ui| {
                            let content = inner_width - 52.0;
                            ui.set_max_width(content);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&reply.author)
                                        .font(theme::semibold(13.0))
                                        .color(palette.text),
                                );
                                ui.label(
                                    egui::RichText::new(crate::time::clock(reply.created))
                                        .font(theme::regular(11.0))
                                        .color(palette.dim),
                                );
                            });
                            body(ui, app, conversation, reply, content, actions);
                            if !reply.reactions.is_empty() {
                                reactions(ui, app, conversation, reply, me, actions);
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
                if expanded
                    && count > 2
                    && ui
                        .link(
                            egui::RichText::new("Collapse")
                                .font(theme::regular(12.0))
                                .color(palette.secondary),
                        )
                        .clicked()
                {
                    actions.push(Action::ToggleThread(root.id.clone()));
                }
            }
            ui.horizontal(|ui| {
                let replying = app
                    .drafts
                    .get(conversation)
                    .and_then(|d| d.reply_to.as_deref())
                    == Some(root.id.as_str());
                let label = if replying {
                    "Replying below…"
                } else {
                    "Reply"
                };
                let response = widgets::text_button(ui, palette, label, theme::medium(12.0));
                if response.clicked() {
                    actions.push(Action::StartReply(root.id.clone()));
                }
            });
        });
    ui.add_space(6.0);
    response.response.rect
}

/// `14:05` today, `Yesterday 14:05`, `Mon 14:05`, `2 Oct 14:05`.
fn stamp(at: i64, now: i64) -> String {
    if crate::time::day_key(at) == crate::time::day_key(now) {
        crate::time::clock(at)
    } else {
        format!(
            "{} {}",
            crate::time::short_label(at, now),
            crate::time::clock(at)
        )
    }
}
