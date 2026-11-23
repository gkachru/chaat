//! The virtualized message history and its dividers.

use super::composer::channel_consent;
use super::message::{Placement, hover_toolbar, message_row};
use super::thread::thread_card;
use super::*;

/// A guess at a row's height before it is laid out.
fn estimate(message: &Message, width: f32, grouped: bool) -> f32 {
    let chars = message.body.to_plain().chars().count() as f32;
    let per_line = (bubble_widths(width).1 / 7.2).max(20.0);
    let lines = (chars / per_line).ceil().max(1.0)
        + message.body.blocks.len().saturating_sub(1) as f32 * 0.4;
    let mut height = lines * 20.0 + 18.0;
    if !grouped {
        height += HEADER + 10.0;
    }
    if !message.reactions.is_empty() {
        height += 34.0;
    }
    if message.quote().is_some() {
        height += 54.0;
    }
    height += message.body.images().count() as f32 * 180.0;
    for attachment in &message.attachments {
        if attachment.kind == AttachmentKind::Card {
            height += match attachment.content.as_deref().and_then(crate::cards::parse) {
                Some(card) => {
                    40.0 + 24.0 * card.body.len() as f32 + 32.0 * card.actions.len() as f32
                }
                None => 24.0,
            };
        }
    }
    height
}

pub(super) fn history(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
    let palette = app.palette;
    let channel = conversation.is_channel();
    let empty = app
        .conversation(conversation)
        .is_none_or(|c| c.messages.is_empty());
    if empty {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if channel && matches!(app.channel_access, Access::NeedsConsent | Access::Forbidden) {
                channel_consent(app, ui, conversation);
            } else if app
                .loading
                .contains(&crate::sync::Resource::Messages(conversation.clone()))
            {
                widgets::empty_state(ui, &palette, Icon::Loader, "Loading messages…", "");
            } else {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::Chat,
                    "No messages yet",
                    "Say hello below.",
                );
            }
        });
        return;
    }
    let me = app.me().to_owned();
    let now = crate::time::now();
    let mut actions: Vec<Action> = Vec::new();
    let jump = app.jump_to.take();
    // A jump to a loaded message wins over the end-scroll of an opening.
    if jump.as_ref().is_some_and(|id| {
        app.conversations
            .get(conversation)
            .is_some_and(|view| view.messages.iter().any(|m| &m.id == id))
    }) {
        app.scroll_to_end = 0;
    }
    let hover_id = egui::Id::new(("chaat-hover-row", conversation.key()));
    let previous_hover: Option<(String, egui::Rect)> = ui.ctx().data(|d| d.get_temp(hover_id));
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let menu_id = egui::Id::new(("chaat-message-menu", conversation.key()));
    let menu_open = egui::Popup::is_id_open(ui.ctx(), menu_id)
        || egui::Popup::is_id_open(ui.ctx(), menu_id.with("react-popup"));
    let mut hovered: Option<(usize, egui::Rect, egui::Rect, bool)> = None;
    let scroll_to_end = app.scroll_to_end > 0;
    app.scroll_to_end = app.scroll_to_end.saturating_sub(1);

    egui::CentralPanel::no_frame().show(ui, |ui| {
        let Some(view) = app.conversations.get_mut(conversation) else {
            return;
        };
        let more = view.more;
        let mut restore = std::mem::take(&mut view.restore_history_anchor)
            .then(|| view.history_anchor.clone())
            .flatten();
        let previous_anchor = view.history_anchor.clone();
        let at_end_id = egui::Id::new(("chaat-history-at-end", conversation.key()));
        let at_end = ui.ctx().data(|d| d.get_temp(at_end_id)).unwrap_or(true);
        let mut anchor = None;
        let mut heights = std::mem::take(&mut view.heights);
        let mut layout = std::mem::take(&mut view.layout);
        let expanded = view.expanded.clone();
        let divider = if channel {
            None
        } else {
            view.place_divider(&me)
        };
        let messages = std::mem::take(&mut view.messages);
        let app_ro: &App = app;
        // Opening a conversation shows its newest messages; from then on egui
        // keeps the view at the end while it is there.
        let output = egui::ScrollArea::vertical()
            .id_salt(("history", conversation.key()))
            .auto_shrink([false, false])
            .stick_to_bottom(jump.is_none())
            .show(ui, |ui| {
                let clip = ui.clip_rect();
                let scroll_spacing = ui.spacing().item_spacing.y;
                let margin = (clip.height() * 2.0).max(600.0);
                Frame::new()
                    .inner_margin(Margin {
                        left: 16,
                        right: 20,
                        top: 12,
                        bottom: 12,
                    })
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        // A new width measures every row again, and the
                        // estimates above the view would move it. Away
                        // from the end, keep the first visible row where
                        // it was; at the end, egui keeps the end.
                        if restore.is_none()
                            && !at_end
                            && layout.resized(width, theme::font_revision(ui.ctx()))
                        {
                            restore = previous_anchor.clone();
                        }
                        if more {
                            ui.vertical_centered(|ui| {
                                let response = widgets::secondary_button(
                                    ui,
                                    &palette,
                                    "Load earlier messages",
                                );
                                widgets::on_click(
                                    &response,
                                    &mut actions,
                                    Action::LoadOlder(conversation.clone()),
                                );
                                // Reaching the top loads more on its own.
                                if ui.is_rect_visible(response.rect)
                                    && !app_ro.loading.contains(&crate::sync::Resource::Messages(
                                        conversation.clone(),
                                    ))
                                    && ui.input(|i| i.smooth_scroll_delta.y > 0.0)
                                {
                                    actions.push(Action::LoadOlder(conversation.clone()));
                                }
                            });
                            ui.add_space(12.0);
                        }
                        layout.prepare(
                            &messages,
                            width,
                            theme::font_revision(ui.ctx()),
                            channel,
                            &mut heights,
                            |message, grouped| estimate(message, width, grouped),
                        );
                        let origin = ui.cursor().top();
                        let visible = layout.visible(
                            clip.top() - origin - margin,
                            clip.bottom() - origin + margin,
                        );
                        let target_id =
                            jump.as_ref().or_else(|| restore.as_ref().map(|(id, _)| id));
                        let target_index =
                            target_id.and_then(|id| messages.iter().position(|m| &m.id == id));
                        let mut ranges = vec![visible];
                        if let Some(index) = target_index {
                            ranges.push(index..index + 1);
                        }
                        ranges.sort_by_key(|range| range.start);
                        let mut end = 0;
                        for range in ranges {
                            let start = range.start.max(end);
                            if start >= range.end {
                                continue;
                            }
                            let skipped = layout.prefix(start) - (ui.cursor().top() - origin);
                            if skipped > 0.0 {
                                ui.add_space(skipped);
                            }
                            for (index, message) in
                                messages.iter().enumerate().take(range.end).skip(start)
                            {
                                let (new_day, grouped) = layout.flags(index);
                                if new_day {
                                    day_divider(ui, app_ro, message.created, now);
                                }
                                let top = ui.cursor().top();
                                let target = target_index == Some(index);
                                // The divider is part of the row's measured
                                // height, and starts a new group.
                                let grouped = if divider == Some(index) {
                                    unread_divider(ui, &palette);
                                    false
                                } else {
                                    grouped
                                };
                                let (rect, bubble) = if channel {
                                    let rect = thread_card(
                                        ui,
                                        app_ro,
                                        conversation,
                                        message,
                                        &me,
                                        now,
                                        expanded.contains(&message.id),
                                        &mut actions,
                                    );
                                    (rect, None)
                                } else {
                                    let (rect, bubble) = message_row(
                                        ui,
                                        app_ro,
                                        conversation,
                                        message,
                                        &me,
                                        grouped,
                                        target,
                                        width,
                                        &mut actions,
                                    );
                                    (rect, Some(bubble))
                                };
                                let measured = ui.cursor().top() - top;
                                heights.insert(message.id.clone(), measured);
                                layout.measure(index, measured);
                                if jump.as_deref() == Some(message.id.as_str()) {
                                    ui.scroll_to_rect(rect, Some(Align::Center));
                                }
                                if let Some((id, offset)) = &restore
                                    && id == &message.id
                                {
                                    ui.scroll_to_rect_animation(
                                        egui::Rect::from_min_size(
                                            // ScrollArea adds item spacing to an
                                            // aligned target; cancel that here.
                                            rect.min - vec2(0.0, *offset - scroll_spacing),
                                            vec2(1.0, clip.height()),
                                        ),
                                        Some(Align::TOP),
                                        egui::style::ScrollAnimation::none(),
                                    );
                                }
                                if anchor.is_none()
                                    && rect.bottom() >= clip.top()
                                    && rect.top() <= clip.bottom()
                                {
                                    anchor = Some((message.id.clone(), rect.top() - clip.top()));
                                }
                                if !message.system && !message.deleted {
                                    // The toolbar, and its menu while open,
                                    // keep their message hovered.
                                    let keep =
                                        previous_hover.as_ref().is_some_and(|(id, toolbar)| {
                                            id == &message.id
                                                && (menu_open
                                                    || pointer.is_some_and(|p| toolbar.contains(p)))
                                        });
                                    let kept = hovered.is_some_and(|(i, ..)| {
                                        menu_open
                                            && previous_hover
                                                .as_ref()
                                                .is_some_and(|(id, _)| id == &messages[i].id)
                                    });
                                    if keep
                                        || (!kept
                                            && pointer.is_some_and(|p| {
                                                // In chats only the bubble itself.
                                                bubble.unwrap_or(rect).contains(p)
                                                    && clip.contains(p)
                                            }))
                                    {
                                        hovered = Some((
                                            index,
                                            rect,
                                            bubble.unwrap_or(rect),
                                            message.is_from(&me),
                                        ));
                                    }
                                }
                            }
                            end = range.end;
                        }
                        let remaining = layout.total() - (ui.cursor().top() - origin);
                        if remaining > 0.0 {
                            ui.add_space(remaining);
                        }
                        ui.add_space(4.0);
                        if scroll_to_end && jump.is_none() {
                            let end = egui::Rect::from_min_size(ui.cursor().min, vec2(1.0, 1.0));
                            ui.scroll_to_rect_animation(
                                end,
                                Some(Align::BOTTOM),
                                egui::style::ScrollAnimation::none(),
                            );
                        }
                        // The bar waits until the pointer rests on a
                        // message, so scrolling past bubbles shows nothing.
                        // A bar already showing stays.
                        let since_id = egui::Id::new(("chaat-hover-since", conversation.key()));
                        let now = ui.input(|i| i.time);
                        let scrolling = ui.input(|i| i.smooth_scroll_delta.y != 0.0);
                        let hovered = hovered.filter(|(index, ..)| {
                            let id = &messages[*index].id;
                            if !scrolling
                                && previous_hover
                                    .as_ref()
                                    .is_some_and(|(shown, _)| shown == id)
                            {
                                return true;
                            }
                            let since: Option<(String, f64)> =
                                ui.ctx().data(|d| d.get_temp(since_id));
                            match since {
                                Some((resting, at)) if &resting == id && !scrolling => {
                                    let waited = now - at;
                                    if waited < HOVER_DELAY {
                                        ui.ctx().request_repaint_after(
                                            std::time::Duration::from_secs_f64(
                                                HOVER_DELAY - waited,
                                            ),
                                        );
                                    }
                                    waited >= HOVER_DELAY
                                }
                                _ => {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(since_id, (id.clone(), now));
                                    });
                                    ui.ctx().request_repaint_after(
                                        std::time::Duration::from_secs_f64(HOVER_DELAY),
                                    );
                                    false
                                }
                            }
                        });
                        if let Some((index, rect, anchor, mine)) = &hovered {
                            let message = &messages[*index];
                            let id = &message.id;
                            let toolbar = hover_toolbar(
                                ui,
                                app_ro,
                                conversation,
                                id,
                                Some(message),
                                Placement {
                                    row: *rect,
                                    bubble: (!channel).then_some(*anchor),
                                },
                                *mine,
                                menu_id,
                                &mut actions,
                            );
                            ui.ctx()
                                .data_mut(|d| d.insert_temp(hover_id, (id.clone(), toolbar)));
                        } else {
                            ui.ctx()
                                .data_mut(|d| d.remove::<(String, egui::Rect)>(hover_id));
                        }
                    });
            });
        // Whether the view sits at the end, for the next frame.
        let at_end =
            output.state.offset.y + output.inner_rect.height() >= output.content_size.y - 2.0;
        ui.ctx().data_mut(|d| d.insert_temp(at_end_id, at_end));
        let far = find::jump_visible(
            output.content_size.y,
            output.state.offset.y,
            output.inner_rect.height(),
        );
        find::jump_button(
            ui,
            &palette,
            conversation,
            output.inner_rect,
            far,
            &mut actions,
        );
        if let Some(view) = app.conversations.get_mut(conversation) {
            if view.messages.is_empty() {
                view.messages = messages;
            } else {
                // Messages arrived while drawing (they cannot: actions run
                // after the frame), keep both to be safe.
                let mut merged = messages;
                merged.append(&mut view.messages);
                view.messages = merged;
            }
            view.heights = heights;
            view.layout = layout;
            if restore.is_none() {
                view.history_anchor = anchor;
            }
        }
    });
    app.actions.extend(actions);
}

fn day_divider(ui: &mut egui::Ui, app: &App, at: i64, now: i64) {
    let palette = &app.palette;
    ui.add_space(10.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        crate::time::day_label(at, now),
        theme::regular(12.0),
        palette.secondary,
    );
    ui.add_space(6.0);
}

/// "Last read" over a line, before the first message the person had not
/// seen when they opened the chat.
fn unread_divider(ui: &mut egui::Ui, palette: &theme::Palette) {
    ui.add_space(12.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    let galley = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap("Last read".into(), theme::semibold(12.0), palette.accent));
    let center = rect.center();
    let half = galley.size().x / 2.0 + 16.0;
    let stroke = Stroke::new(1.0, palette.outline);
    ui.painter()
        .hline(rect.left() + GUTTER..=center.x - half, center.y, stroke);
    ui.painter()
        .hline(center.x + half..=rect.right(), center.y, stroke);
    ui.painter()
        .galley(center - galley.size() / 2.0, galley, palette.accent);
    ui.add_space(2.0);
}

/// A system event's icon and words, as Teams shows them: people added or
/// removed by name, and calls with their time and, in bold, how long they
/// lasted. Names Graph left out are looked up.
pub(super) fn system_line(
    app: &App,
    message: &Message,
    wrap: f32,
    actions: &mut Vec<Action>,
) -> (Option<Icon>, egui::text::LayoutJob) {
    use crate::model::MemberChange;
    let palette = &app.palette;
    let format = |font: egui::FontId, color: egui::Color32| egui::TextFormat {
        font_id: font,
        color,
        valign: Align::Center,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap;
    let words = format(theme::regular(13.0), palette.secondary);
    if let Some(membership) = &message.membership {
        let unknown: Vec<String> = membership
            .unnamed()
            .filter(|id| app.needs_name(id))
            .map(str::to_owned)
            .collect();
        if !unknown.is_empty() {
            actions.push(Action::LookUpNames(unknown));
        }
        job.append(&membership.describe(|id| app.person_name(id)), 0.0, words);
        let icon = match membership.change {
            MemberChange::Added | MemberChange::Joined => Icon::UserPlus,
            MemberChange::Removed | MemberChange::Left => Icon::UserMinus,
        };
        return (Some(icon), job);
    }
    if let Some(call) = &message.call {
        let now = crate::time::now();
        let clock = crate::time::clock(message.created);
        let at = if crate::time::day_key(message.created) == crate::time::day_key(now) {
            clock
        } else {
            format!("{} {clock}", crate::time::day_label(message.created, now))
        };
        job.append(&at, 0.0, format(theme::regular(12.0), palette.dim));
        let what = if call.meeting { "Meeting" } else { "Call" };
        let words_text = if message.missed_call {
            "Missed call".to_owned()
        } else if call.ended {
            format!("{what} ended")
        } else {
            format!("{what} started")
        };
        job.append(&words_text, 12.0, words.clone());
        if let Some(duration) = call.duration.as_deref().filter(|_| !message.missed_call) {
            job.append(duration, 6.0, format(theme::semibold(13.0), palette.text));
        }
        let icon = if call.meeting {
            Icon::Video
        } else {
            Icon::Phone
        };
        return (Some(icon), job);
    }
    job.append(&message.body.to_plain(), 0.0, words);
    (None, job)
}

/// Whom "Call back" on a missed call rings: everyone else in the chat, as
/// the chat header's call buttons do. Empty outside chats or without
/// addresses, which hides the button.
pub(super) fn missed_call_back(app: &App, conversation: &ConversationId, me: &str) -> Vec<String> {
    let ConversationId::Chat(id) = conversation else {
        return Vec::new();
    };
    app.chat(id).map_or_else(Vec::new, |chat| {
        chat.members
            .iter()
            .filter(|m| m.user_id != me)
            .filter_map(|m| m.email.clone())
            .collect()
    })
}
