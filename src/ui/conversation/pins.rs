//! The pinned-message banner under the header.

use super::*;

/// The icon of pinned messages.
pub(super) const PIN_ICON: Icon = Icon::Pin;

/// The pin shown after stepping `delta` (1 or -1) from `index` among `len`.
fn step_pin(index: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (index.min(len - 1) as isize + delta).rem_euclid(len as isize) as usize
}

fn banner_bottom_id() -> egui::Id {
    egui::Id::new("pin-banner-bottom")
}

/// The bottom edge of the pin banner when this frame drew one. Cards that
/// float over the conversation's top right start below it, so they never
/// cover its controls.
pub fn pin_banner_bottom(ctx: &egui::Context) -> Option<f32> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, f32)>(banner_bottom_id()))
        .filter(|(drawn, _)| *drawn == pass)
        .map(|(_, bottom)| bottom)
}

/// One line under the header for the open chat's pinned messages: the
/// current pin, arrows to step through them, and Unpin.
pub(super) fn pin_banner(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
    let ConversationId::Chat(chat) = conversation else {
        return;
    };
    let pins = app.pins.get(chat).cloned().unwrap_or_default();
    if pins.is_empty() {
        return;
    }
    let palette = app.palette;
    let state = egui::Id::new(("pin-index", chat));
    let index = ui
        .data(|d| d.get_temp::<usize>(state))
        .unwrap_or(0)
        .min(pins.len() - 1);
    let pin = &pins[index];
    let mut actions = Vec::new();
    let mut next_index = index;
    let banner = egui::Panel::top(egui::Id::new(("pin-banner", chat)))
        .exact_size(34.0)
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(palette.window)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter().hline(
                rect.x_range().expand(16.0),
                rect.bottom(),
                Stroke::new(1.0, palette.outline),
            );
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (icon, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                PIN_ICON.image(palette.secondary, 14.0).paint_at(ui, icon);
                let mut jump = false;
                // Right to left: Unpin, arrows and count, then the text takes
                // the rest.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if widgets::text_button(ui, &palette, "Unpin", theme::regular(12.0)).clicked() {
                        actions.push(Action::UnpinMessage {
                            chat: chat.clone(),
                            pin: pin.id.clone(),
                        });
                    }
                    if pins.len() > 1 {
                        let next = widgets::icon_button_sized(
                            ui,
                            &palette,
                            Icon::ChevronRight,
                            "Next pinned message",
                            24.0,
                            14.0,
                            palette.secondary,
                        );
                        if next.clicked() {
                            next_index = step_pin(index, pins.len(), 1);
                        }
                        ui.label(
                            egui::RichText::new(format!("{} of {}", index + 1, pins.len()))
                                .font(theme::regular(12.0))
                                .color(palette.dim),
                        );
                        let prev = widgets::icon_button_sized(
                            ui,
                            &palette,
                            Icon::ChevronLeft,
                            "Previous pinned message",
                            24.0,
                            14.0,
                            palette.secondary,
                        );
                        if prev.clicked() {
                            next_index = step_pin(index, pins.len(), -1);
                        }
                    }
                    // Author and preview as one label truncated to the room
                    // the controls leave, so it never runs under them. (A
                    // horizontal layout would make egui ignore the job's own
                    // truncation.)
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let mut job = egui::text::LayoutJob::default();
                        let format = |font, color| egui::TextFormat {
                            font_id: font,
                            color,
                            ..Default::default()
                        };
                        // With little room the author would take all of it and
                        // hide the message; then only the message shows.
                        let author_width = ui
                            .painter()
                            .layout_no_wrap(pin.author.clone(), theme::semibold(12.0), palette.text)
                            .size()
                            .x;
                        let with_author = ui.available_width() >= author_width + 6.0 + 90.0;
                        if with_author {
                            job.append(
                                &pin.author,
                                0.0,
                                format(theme::semibold(12.0), palette.text),
                            );
                        }
                        job.append(
                            &pin.preview.replace('\n', " "),
                            if with_author { 6.0 } else { 0.0 },
                            format(theme::regular(12.0), palette.secondary),
                        );
                        let text = ui
                            .add(
                                egui::Label::new(job)
                                    .truncate()
                                    .selectable(false)
                                    .sense(Sense::click()),
                            )
                            .on_hover_text(format!("{}: {}", pin.author, pin.preview))
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        jump = text.clicked();
                    });
                });
                if jump {
                    actions.push(Action::OpenMessage {
                        conversation: conversation.clone(),
                        message: pin.message_id.clone(),
                    });
                }
            });
        });
    let pass = ui.ctx().cumulative_pass_nr();
    ui.data_mut(|d| d.insert_temp(banner_bottom_id(), (pass, banner.response.rect.bottom())));
    if next_index != index {
        ui.data_mut(|d| d.insert_temp(state, next_index));
    }
    app.actions.extend(actions);
}

#[cfg(test)]
mod pin_tests {
    use super::step_pin;

    #[test]
    fn pin_index_wraps_both_ways() {
        assert_eq!(step_pin(0, 3, 1), 1);
        assert_eq!(step_pin(2, 3, 1), 0);
        assert_eq!(step_pin(0, 3, -1), 2);
        assert_eq!(step_pin(5, 3, 1), 0);
        assert_eq!(step_pin(0, 0, 1), 0);
    }
}
