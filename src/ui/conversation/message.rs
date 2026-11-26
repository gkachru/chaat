//! One message: bubble, body, attachments, reactions and the hover toolbar.

use super::history::{missed_call_back, system_line};
use super::pins::PIN_ICON;
use super::*;

/// One chat message as a bubble: on the right for the person's own
/// messages, on the left under the author's name and picture for everyone
/// else's. Returns the row's rect and the bubble's.
#[allow(clippy::too_many_arguments)]
pub(super) fn message_row(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    message: &Message,
    me: &str,
    grouped: bool,
    highlighted: bool,
    width: f32,
    actions: &mut Vec<Action>,
) -> (egui::Rect, egui::Rect) {
    let palette = &app.palette;
    if let Some(recording) = &message.recording {
        return super::recording::recording_row(ui, app, message, recording, width, actions);
    }
    if message.system {
        let call_back = if message.missed_call {
            missed_call_back(app, conversation, me)
        } else {
            Vec::new()
        };
        // Leave room for Call back, so a long date (older than yesterday)
        // wraps instead of pushing the button out of the view.
        let button_width = if !call_back.is_empty() {
            CALL_BACK_GAP + widgets::pill_width(ui, "Call back")
        } else {
            0.0
        };
        // A pill is taller than the line, so lift the icon and words to its
        // vertical centre.
        let lift = if button_width > 0.0 {
            (widgets::PILL_HEIGHT - 20.0) / 2.0
        } else {
            0.0
        };
        let text_width = (width - GUTTER - 8.0 - button_width).max(80.0);
        let (icon, job) = system_line(app, message, text_width, actions);
        let response = ui.allocate_ui(vec2(width, 0.0), |ui| {
            ui.set_width(width);
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // The icon sits in the avatar column, the words where
                // messages start.
                let (gutter, _) = ui.allocate_exact_size(vec2(GUTTER, 20.0 + lift), Sense::hover());
                if let Some(icon) = icon {
                    icon.image(palette.dim, 18.0).paint_at(
                        ui,
                        egui::Rect::from_min_size(
                            gutter.min + vec2(7.0, 1.0 + lift),
                            egui::Vec2::splat(18.0),
                        ),
                    );
                }
                let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
                ui.vertical(|ui| {
                    ui.add_space(lift);
                    ui.add(egui::Label::new(galley))
                        .on_hover_text(crate::time::day_label(message.created, crate::time::now()));
                });
                if !call_back.is_empty() {
                    ui.add_space(CALL_BACK_GAP);
                    let response = widgets::pill(ui, palette, Icon::Phone, "Call back", false)
                        .on_hover_text("Call back in Microsoft Teams");
                    widgets::on_click(
                        &response,
                        actions,
                        Action::StartCall {
                            users: call_back,
                            video: false,
                        },
                    );
                }
            });
            ui.add_space(6.0);
        });
        let rect = response.response.rect;
        return (rect, rect);
    }
    let mentioned = message.body.blocks.iter().any(|block| match block {
        crate::html::Block::Paragraph(spans) => spans.iter().any(|s| s.style.mention),
        _ => false,
    }) && app
        .profile
        .as_ref()
        .is_some_and(|p| message.body.to_plain().contains(&p.display_name));
    let mine = message.is_from(me);
    let stroke = if message.importance_high {
        Stroke::new(1.0, palette.danger)
    } else if mentioned {
        Stroke::new(1.0, palette.accent)
    } else {
        Stroke::NONE
    };
    let fill = if mine {
        palette.bubble_mine()
    } else {
        palette.bubble()
    };
    let (bubble_max, text_width) = bubble_widths(width);
    let start = ui.cursor().min;
    let behind = ui.painter().add(egui::Shape::Noop);
    let mut bubble = egui::Rect::NOTHING;
    let response = ui.allocate_ui(vec2(width, 0.0), |ui| {
        ui.set_width(width);
        ui.add_space(if grouped { 2.0 } else { 10.0 });
        ui.spacing_mut().item_spacing.y = 4.0;
        if mine {
            if !grouped {
                let (line, _) = ui.allocate_exact_size(vec2(width, HEADER), Sense::hover());
                header_line(ui, palette, message, line, true);
            }
            bubble = right_aligned(
                ui,
                egui::Id::new(("chaat-bubble", &message.id)),
                bubble_max,
                |ui| {
                    bubble_frame(
                        ui,
                        app,
                        conversation,
                        message,
                        (fill, stroke),
                        text_width,
                        actions,
                    )
                },
            );
            if !message.reactions.is_empty() {
                // The pills tuck 1 point into the bubble's bottom edge.
                ui.add_space(-(ui.spacing().item_spacing.y + REACTION_OVERLAP));
                right_aligned(
                    ui,
                    egui::Id::new(("chaat-reactions", &message.id)),
                    bubble_max,
                    |ui| reactions(ui, app, conversation, message, me, actions),
                );
            }
            let pills = crate::ui::agents::pending_reactions(app, conversation, &message.id);
            if !pills.is_empty() {
                right_aligned(
                    ui,
                    egui::Id::new(("chaat-agent-pills", &message.id)),
                    bubble_max,
                    |ui| crate::ui::agents::reaction_pills(ui, app, &pills, actions),
                );
            }
        } else {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (gutter, _) = ui.allocate_exact_size(
                    vec2(GUTTER, if grouped { 1.0 } else { HEADER + AVATAR }),
                    Sense::hover(),
                );
                if !grouped {
                    let mut child =
                        ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                            gutter.min + vec2(0.0, HEADER + 2.0),
                            egui::Vec2::splat(AVATAR),
                        )));
                    widgets::avatar(
                        &mut child,
                        app,
                        message.author_id.as_deref(),
                        &message.author,
                        AVATAR,
                        true,
                    );
                }
                ui.vertical(|ui| {
                    ui.set_max_width(bubble_max);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    if !grouped {
                        let (line, _) =
                            ui.allocate_exact_size(vec2(bubble_max, HEADER), Sense::hover());
                        header_line(ui, palette, message, line, false);
                    }
                    bubble = bubble_frame(
                        ui,
                        app,
                        conversation,
                        message,
                        (fill, stroke),
                        text_width,
                        actions,
                    );
                    if !message.reactions.is_empty() {
                        // The pills tuck 1 point into the bubble's bottom edge.
                        ui.add_space(-(ui.spacing().item_spacing.y + REACTION_OVERLAP));
                        reactions(ui, app, conversation, message, me, actions);
                    }
                    let pills =
                        crate::ui::agents::pending_reactions(app, conversation, &message.id);
                    if !pills.is_empty() {
                        crate::ui::agents::reaction_pills(ui, app, &pills, actions);
                    }
                });
            });
        }
        ui.add_space(1.0);
    });
    let rect = egui::Rect::from_min_max(
        start,
        egui::pos2(start.x + width, response.response.rect.bottom()),
    );
    // A grouped message shows its time beside the bubble on hover.
    if grouped && ui.rect_contains_pointer(rect) {
        let (at, align) = if mine {
            (
                egui::pos2(bubble.left() - 8.0, bubble.center().y),
                egui::Align2::RIGHT_CENTER,
            )
        } else {
            (
                egui::pos2(rect.left() + 2.0, bubble.center().y),
                egui::Align2::LEFT_CENTER,
            )
        };
        ui.painter().text(
            at,
            align,
            crate::time::clock(message.created),
            theme::regular(11.0),
            palette.dim,
        );
    }
    if highlighted {
        ui.painter().set(
            behind,
            egui::Shape::rect_filled(
                rect.expand2(vec2(8.0, 0.0)),
                CornerRadius::same(6),
                palette.highlight,
            ),
        );
    }
    find::outline_current(ui, app, &message.id, bubble);
    (rect, bubble)
}

/// The author, time and importance over a group of messages. The person's
/// own messages show only the time, on the right.
fn header_line(
    ui: &egui::Ui,
    palette: &theme::Palette,
    message: &Message,
    line: egui::Rect,
    mine: bool,
) {
    let mut parts = Vec::new();
    if !mine {
        parts.push((
            message.author.clone(),
            theme::regular(12.0),
            palette.secondary,
        ));
    }
    parts.push((
        crate::time::clock(message.created),
        theme::regular(12.0),
        palette.dim,
    ));
    if message.importance_high {
        parts.push(("IMPORTANT".to_owned(), theme::bold(11.0), palette.danger));
    }
    let gap = 8.0;
    let galleys: Vec<_> = parts
        .into_iter()
        .map(|(text, font, color)| {
            (
                ui.ctx().fonts_mut(|f| f.layout_no_wrap(text, font, color)),
                color,
            )
        })
        .collect();
    let total = galleys.iter().map(|(g, _)| g.size().x).sum::<f32>()
        + gap * galleys.len().saturating_sub(1) as f32;
    let mut x = if mine {
        line.right() - total
    } else {
        line.left()
    };
    let time_index = usize::from(!mine);
    for (index, (galley, color)) in galleys.into_iter().enumerate() {
        let size = galley.size();
        let at = egui::pos2(x, line.bottom() - size.y - 2.0);
        if index == time_index {
            ui.interact(
                egui::Rect::from_min_size(at, size),
                egui::Id::new(("chaat-time", &message.id)),
                Sense::hover(),
            )
            .on_hover_text(crate::time::day_label(message.created, crate::time::now()));
        }
        ui.painter().galley(at, galley, color);
        x += size.x + gap;
    }
}

/// Lays `add` out against the right edge of `ui`. Its width is known only
/// afterwards, so it comes from the frame before; the first frame lays it
/// out invisibly and asks for one more.
fn right_aligned<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    max_width: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let known: Option<f32> = ui.ctx().data(|d| d.get_temp(id));
    let right = ui.max_rect().right();
    let left = right - known.unwrap_or(max_width).min(max_width);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                egui::pos2(left, ui.cursor().top()),
                vec2(max_width, 0.0),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    if known.is_none() {
        child.set_invisible();
    }
    let inner = add(&mut child);
    let used = child.min_rect();
    if known.is_none_or(|k| (k - used.width()).abs() > 0.5) {
        ui.ctx().data_mut(|d| d.insert_temp(id, used.width()));
        ui.ctx().request_repaint();
    }
    // Content wider than the row (for a frame, before it is measured) must
    // not widen the history: every later row would take that width.
    ui.advance_cursor_after_rect(egui::Rect::from_min_max(
        used.min,
        egui::pos2(used.max.x.min(right), used.max.y),
    ));
    inner
}

/// A message's bubble. Returns its rect.
#[allow(clippy::too_many_arguments)]
fn bubble_frame(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    message: &Message,
    (fill, stroke): (egui::Color32, Stroke),
    text_width: f32,
    actions: &mut Vec<Action>,
) -> egui::Rect {
    Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(BUBBLE_PAD)
        .show(ui, |ui| {
            body(ui, app, conversation, message, text_width, actions);
        })
        .response
        .rect
}

/// A message's content below its header: quote, text, attachments,
/// delivery state. Reactions go below it.
pub(super) fn body(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    message: &Message,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    if message.deleted {
        ui.label(
            egui::RichText::new("This message was deleted.")
                .italics()
                .color(palette.dim),
        );
        return;
    }
    if let Some(quote) = message.quote()
        && let AttachmentKind::Quote {
            author,
            preview,
            message_id,
        } = &quote.kind
    {
        let response = rich::quote_block(ui, palette, author.as_deref(), preview, width.min(560.0));
        let response = response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        widgets::hover_wash(ui, palette, &response, 6);
        if response.clicked() {
            actions.push(Action::OpenMessage {
                conversation: conversation.clone(),
                message: message_id.clone(),
            });
        }
    }
    let look = rich::Look {
        size: 14.0,
        color: if message.delivery == Delivery::Sent {
            palette.text
        } else {
            palette.secondary
        },
        // Quotes, files and cards make it an ordinary message.
        jumbo: message.attachments.is_empty() && rich::body_is_emoji_only(&message.body),
    };
    rich::show(ui, app, &message.body, &look, width, actions);
    for attachment in &message.attachments {
        match &attachment.kind {
            AttachmentKind::File => file_chip(ui, app, attachment, actions),
            AttachmentKind::Card => {
                if let Some(card) = attachment.content.as_deref().and_then(crate::cards::parse) {
                    crate::ui::cards::show(ui, palette, &card, width, actions);
                    continue;
                }
                ui.label(
                    egui::RichText::new(
                        "A card Chaat can't show yet. Open the message in Teams to see it.",
                    )
                    .font(theme::regular(12.0))
                    .color(palette.dim),
                );
            }
            _ => {}
        }
    }
    if message.edited {
        ui.label(
            egui::RichText::new("Edited")
                .font(theme::regular(11.0))
                .color(palette.dim),
        );
    }
    match message.delivery {
        Delivery::Sending => {
            ui.label(
                egui::RichText::new("Sending…")
                    .font(theme::regular(11.0))
                    .color(palette.dim),
            );
        }
        Delivery::Failed => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Not sent.")
                        .font(theme::regular(12.0))
                        .color(palette.danger),
                );
                if ui
                    .link(egui::RichText::new("Retry").font(theme::semibold(12.0)))
                    .clicked()
                {
                    actions.push(Action::RetrySend {
                        conversation: conversation.clone(),
                        local_id: message.id.clone(),
                    });
                }
                ui.label(
                    egui::RichText::new("·")
                        .font(theme::regular(12.0))
                        .color(palette.dim),
                );
                if ui
                    .link(egui::RichText::new("Delete").font(theme::semibold(12.0)))
                    .clicked()
                {
                    actions.push(Action::DeleteFailed {
                        conversation: conversation.clone(),
                        local_id: message.id.clone(),
                    });
                }
            });
        }
        Delivery::Sent => {}
    }
}

fn file_chip(
    ui: &mut egui::Ui,
    app: &App,
    attachment: &crate::model::Attachment,
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    let name = attachment
        .name
        .clone()
        .unwrap_or_else(|| "Attachment".into());
    let response = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::File, 18.0, palette.accent);
                // A long name ends in "…" inside the bubble, keeping room
                // for the link icon; it must not widen the history.
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - 20.0).max(40.0));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&name)
                                .font(theme::medium(13.0))
                                .color(palette.text),
                        )
                        .truncate(),
                    );
                });
                if attachment.url.is_some() {
                    theme::icon(ui, Icon::ExternalLink, 13.0, palette.dim);
                }
            });
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Open in your browser");
    widgets::hover_wash(ui, palette, &response, 8);
    if response.clicked()
        && let Some(url) = &attachment.url
    {
        actions.push(Action::OpenUrl(url.clone()));
    }
}

/// How far reaction pills overlap the bottom edge of a chat bubble.
const REACTION_OVERLAP: f32 = 1.0;

/// Reactions under a message: the emoji, with a count when more than one
/// person reacted, the person's own outlined in the accent; then a button
/// to add one.
pub(super) fn reactions(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    message: &Message,
    me: &str,
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for reaction in &message.reactions {
            let mine = reaction.users.iter().any(|u| u == me);
            let mut job = egui::text::LayoutJob::default();
            job.append(
                &reaction.emoji,
                0.0,
                egui::TextFormat {
                    font_id: theme::regular(18.0),
                    color: palette.text,
                    valign: Align::Center,
                    ..Default::default()
                },
            );
            let count = reaction.users.len();
            if count > 1 {
                job.append(
                    &count.to_string(),
                    5.0,
                    egui::TextFormat {
                        font_id: theme::medium(12.0),
                        color: if mine {
                            palette.accent
                        } else {
                            palette.secondary
                        },
                        valign: Align::Center,
                        ..Default::default()
                    },
                );
            }
            let button = egui::Button::new(job)
                .stroke(Stroke::new(
                    1.0,
                    if mine {
                        palette.accent
                    } else {
                        palette.outline
                    },
                ))
                .corner_radius(CornerRadius::same(13))
                .min_size(vec2(32.0, 26.0));
            let names: Vec<String> = reaction
                .users
                .iter()
                .map(|u| {
                    if u == me {
                        "You".to_owned()
                    } else {
                        app.chats
                            .iter()
                            .flat_map(|c| c.members.iter())
                            .find(|m| &m.user_id == u)
                            .map(|m| m.display_name.clone())
                            .unwrap_or_else(|| "Someone".into())
                    }
                })
                .collect();
            let response = widgets::with_fills(ui, palette.bubble(), palette.surface_hover, |ui| {
                // Snug around the emoji: a little room at the sides, almost
                // none above and below.
                ui.spacing_mut().button_padding = vec2(6.0, 0.0);
                ui.add(button)
            })
            .on_hover_text(names.join(", "));
            if response.clicked() {
                actions.push(Action::React {
                    conversation: conversation.clone(),
                    message: message.id.clone(),
                    emoji: reaction.emoji.clone(),
                    on: !mine,
                });
            }
        }
        let add = widgets::with_fills(ui, palette.window, palette.surface_hover, |ui| {
            ui.add(
                egui::Button::image(Icon::Smile.image(palette.secondary, 16.0))
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(13))
                    .min_size(vec2(34.0, 26.0)),
            )
        })
        .on_hover_text("Add a reaction");
        egui::Popup::menu(&add)
            .id(egui::Id::new(("chaat-add-reaction", &message.id)))
            .show(|ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for emoji in QUICK_REACTIONS {
                        let on = message
                            .reactions
                            .iter()
                            .any(|r| r.emoji == emoji && r.users.iter().any(|u| u == me));
                        let response = emoji_button(
                            ui,
                            palette,
                            emoji,
                            vec2(36.0, 34.0),
                            20.0,
                            palette.text,
                            on,
                        );
                        if response.clicked() {
                            actions.push(Action::React {
                                conversation: conversation.clone(),
                                message: message.id.clone(),
                                emoji: emoji.to_owned(),
                                on: !on,
                            });
                        }
                    }
                });
            });
    });
}

/// Where the hover toolbar goes: over a chat bubble's top edge, or at the
/// right end of a channel post.
pub(super) struct Placement {
    pub(super) row: egui::Rect,
    pub(super) bubble: Option<egui::Rect>,
}

/// The actions shown over the hovered message: quick reactions, reply, and
/// a menu with the rest. Returns the toolbar's rect.
#[allow(clippy::too_many_arguments)]
pub(super) fn hover_toolbar(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    id: &str,
    message: Option<&Message>,
    placement: Placement,
    mine: bool,
    menu_id: egui::Id,
    actions: &mut Vec<Action>,
) -> egui::Rect {
    let palette = &app.palette;
    let can_edit =
        mine && (!conversation.is_channel() || app.has_scope("ChannelMessage.ReadWrite"));
    let size = vec2(
        QUICK_REACTIONS.len() as f32 * 32.0 + 28.0 + 9.0 + 2.0 * 28.0 + 6.0,
        36.0,
    );
    let clip = ui.clip_rect();
    let row = placement.row;
    let min = match placement.bubble {
        Some(bubble) => {
            // The bar lines up with the bubble edge nearest the middle:
            // the left of your own, the right of everyone else's.
            let x = if mine {
                bubble.left()
            } else {
                bubble.right() - size.x
            };
            egui::pos2(
                x.clamp(row.left(), (row.right() - size.x).max(row.left())),
                (bubble.top() - size.y + 1.0).max(clip.top() + 2.0),
            )
        }
        None => egui::pos2(
            row.right() - size.x - 4.0,
            (row.top() - 12.0).max(clip.top() + 2.0),
        ),
    };
    let rect = egui::Rect::from_min_size(min, size);
    // Pointing at one quick reaction dims the others. Worked out from the
    // pointer, since the buttons draw left to right before their hover is
    // known.
    let pointed = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| rect.contains(*p))
        .map(|p| ((p.x - rect.left() - 3.0) / 32.0).floor())
        .filter(|i| (0.0..QUICK_REACTIONS.len() as f32).contains(i))
        .map(|i| i as usize);
    let reacted = |emoji: &str| {
        message.is_some_and(|m| {
            m.reactions
                .iter()
                .any(|r| r.emoji == emoji && r.users.iter().any(|u| u == app.me()))
        })
    };
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        Frame::new()
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.outline))
            .corner_radius(CornerRadius::same(8))
            // A deeper shadow lifts the bar off the message it overlaps.
            .shadow(egui::Shadow {
                offset: [0, 4],
                blur: 14,
                spread: 1,
                color: palette.shadow,
            })
            .inner_margin(Margin::same(3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (index, emoji) in QUICK_REACTIONS.into_iter().enumerate() {
                        let on = reacted(emoji);
                        let color = if pointed.is_none_or(|p| p == index) {
                            palette.text
                        } else {
                            palette.text.gamma_multiply(0.35)
                        };
                        let response =
                            emoji_button(ui, palette, emoji, vec2(32.0, 30.0), 18.0, color, on)
                                .on_hover_text(QUICK_REACTION_NAMES[index]);
                        if response.clicked() {
                            actions.push(Action::React {
                                conversation: conversation.clone(),
                                message: id.to_owned(),
                                emoji: emoji.to_owned(),
                                on: !on,
                            });
                        }
                    }
                    let more_reactions = widgets::icon_button_sized(
                        ui,
                        palette,
                        Icon::Plus,
                        "More reactions",
                        28.0,
                        16.0,
                        palette.secondary,
                    );
                    if let Some(emoji) = composer_tools::emoji_popup(
                        palette,
                        &more_reactions,
                        menu_id.with("react-popup"),
                        menu_id.with("react-picker"),
                    ) {
                        let on = !reacted(&emoji);
                        actions.push(Action::React {
                            conversation: conversation.clone(),
                            message: id.to_owned(),
                            emoji,
                            on,
                        });
                    }
                    let (separator, _) = ui.allocate_exact_size(vec2(9.0, 30.0), Sense::hover());
                    ui.painter().vline(
                        separator.center().x,
                        separator.y_range().shrink(5.0),
                        Stroke::new(1.0, palette.outline),
                    );
                    let reply_label = if conversation.is_channel() {
                        "Reply in thread"
                    } else {
                        "Reply"
                    };
                    let reply = widgets::icon_button_sized(
                        ui,
                        palette,
                        Icon::Reply,
                        reply_label,
                        28.0,
                        16.0,
                        palette.secondary,
                    );
                    widgets::on_click(&reply, actions, Action::StartReply(id.to_owned()));
                    let more = widgets::icon_button_sized(
                        ui,
                        palette,
                        Icon::More,
                        "More actions",
                        28.0,
                        16.0,
                        palette.secondary,
                    );
                    egui::Popup::menu(&more).id(menu_id).show(|ui| {
                        ui.set_min_width(160.0);
                        if can_edit {
                            if widgets::menu_item(ui, palette, Icon::Pencil, "Edit") {
                                actions.push(Action::StartEdit(id.to_owned()));
                            }
                            if widgets::menu_item(ui, palette, Icon::Trash, "Delete") {
                                actions.push(Action::Delete {
                                    conversation: conversation.clone(),
                                    message: id.to_owned(),
                                });
                            }
                        }
                        if widgets::menu_item(ui, palette, Icon::Copy, "Copy text")
                            && let Some(message) = message
                        {
                            actions.push(Action::CopyText(message.body.to_plain()));
                        }
                        if let ConversationId::Chat(chat) = conversation
                            && message.is_some_and(|m| !m.system)
                        {
                            let pin = app
                                .pins
                                .get(chat)
                                .and_then(|pins| pins.iter().find(|p| p.message_id == id));
                            match pin {
                                Some(pin) => {
                                    if widgets::menu_item(ui, palette, PIN_ICON, "Unpin") {
                                        actions.push(Action::UnpinMessage {
                                            chat: chat.clone(),
                                            pin: pin.id.clone(),
                                        });
                                    }
                                }
                                None => {
                                    if widgets::menu_item(ui, palette, PIN_ICON, "Pin") {
                                        actions.push(Action::PinMessage {
                                            chat: chat.clone(),
                                            message: id.to_owned(),
                                        });
                                    }
                                }
                            }
                        }
                        if let Some(message) = message
                            && !message.system
                            && crate::app::message_link(
                                conversation,
                                id,
                                message.web_url.as_deref(),
                            )
                            .is_some()
                            && widgets::menu_item(ui, palette, Icon::Link, "Copy link")
                        {
                            actions.push(Action::CopyMessageLink {
                                conversation: conversation.clone(),
                                message: id.to_owned(),
                            });
                        }
                    });
                });
            });
    });
    rect
}

/// A quick-reaction emoji: highlighted when the person has reacted with it,
/// and lit up under the pointer.
fn emoji_button(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    emoji: &str,
    size: egui::Vec2,
    font_size: f32,
    color: egui::Color32,
    on: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        let fill = match (on, hovered) {
            (true, _) => Some(palette.highlight),
            (false, true) => Some(palette.surface_hover),
            (false, false) => None,
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
        }
        if on && hovered {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(6),
                Stroke::new(1.0, palette.outline),
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            emoji,
            egui::FontId::proportional(font_size),
            color,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
