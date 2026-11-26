//! A meeting recording in a meeting chat: a card with the recording's name,
//! when it happened and how long it ran, and an Open button.

use std::collections::HashSet;

use super::*;

/// The card's inner padding, and the tile's side.
const PAD: f32 = 10.0;
const TILE: f32 = 40.0;

/// The indexes of recording events to leave out: Graph posts one while a
/// recording is still being made (no link yet) and one when it is done, so a
/// link-less event with a later message that carries the link is noise. One
/// with none after it stays, as the only sign of the recording.
pub(super) fn hidden_recording_events(messages: &[Message]) -> HashSet<usize> {
    let mut hidden = HashSet::new();
    let mut link_follows = false;
    for (index, message) in messages.iter().enumerate().rev() {
        if message.recording.is_some() {
            link_follows = true;
        } else if message.recording_event && link_follows {
            hidden.insert(index);
        }
    }
    hidden
}

/// One recording, drawn where messages start. Returns the row's rect twice,
/// as `message_row` does.
pub(super) fn recording_row(
    ui: &mut egui::Ui,
    app: &App,
    message: &Message,
    recording: &crate::model::Recording,
    width: f32,
    actions: &mut Vec<Action>,
) -> (egui::Rect, egui::Rect) {
    let palette = &app.palette;
    let card_width = bubble_widths(width).0.min(width - GUTTER).max(0.0);
    let response = ui.allocate_ui(vec2(width, 0.0), |ui| {
        ui.set_width(width);
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(GUTTER);
            let (card, response) =
                ui.allocate_exact_size(vec2(card_width, TILE + 2.0 * PAD), Sense::click());
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            // The pill sits over the card and takes its own clicks, so
            // hover is the pointer being inside, not the card's response.
            let hovered = ui.rect_contains_pointer(card);
            let open = widgets::pill_width(ui, "Open");
            let tile =
                egui::Rect::from_min_size(card.min + vec2(PAD, PAD), egui::Vec2::splat(TILE));
            let text_left = tile.right() + PAD;
            let text_width = (card.right() - PAD - open - PAD - text_left).max(20.0);
            let title = widgets::elided(
                ui,
                &recording.name,
                theme::semibold(14.0),
                palette.text,
                text_width,
            );
            let mut meta = crate::time::at_label(message.created, crate::time::now());
            if let Some(duration) = &recording.duration {
                meta.push_str(" · ");
                meta.push_str(duration);
            }
            let meta = widgets::elided(ui, &meta, theme::regular(12.0), palette.dim, text_width);
            if ui.is_rect_visible(card) {
                let fill = if hovered {
                    palette.bubble().lerp_to_gamma(palette.surface_hover, 0.5)
                } else {
                    palette.bubble()
                };
                ui.painter().rect_filled(card, CornerRadius::same(8), fill);
                widgets::glass_tile(ui.painter(), palette, tile, hovered);
                Icon::Play.image(palette.accent, 20.0).paint_at(
                    ui,
                    egui::Rect::from_center_size(tile.center(), egui::Vec2::splat(20.0)),
                );
                let block = title.size().y + meta.size().y;
                let top = card.center().y - block / 2.0;
                let meta_top = top + title.size().y;
                ui.painter()
                    .galley(egui::pos2(text_left, top), title, palette.text);
                ui.painter()
                    .galley(egui::pos2(text_left, meta_top), meta, palette.dim);
            }
            widgets::on_click(&response, actions, Action::OpenUrl(recording.url.clone()));
            let pill = egui::Rect::from_center_size(
                egui::pos2(card.right() - PAD - open / 2.0, card.center().y),
                vec2(open, widgets::PILL_HEIGHT),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(pill)
                    .layout(egui::Layout::left_to_right(Align::Center)),
            );
            let button = widgets::pill(&mut child, palette, Icon::Play, "Open", false)
                .on_hover_text("Open the recording in your browser");
            widgets::on_click(&button, actions, Action::OpenUrl(recording.url.clone()));
        });
        ui.add_space(6.0);
    });
    let rect = response.response.rect;
    (rect, rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(recording_event: bool, link: bool) -> Message {
        Message {
            recording_event,
            recording: link.then(|| crate::model::Recording {
                url: "https://c-my.sharepoint.com/r.mp4".into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn hidden(messages: &[Message]) -> Vec<usize> {
        let mut found: Vec<usize> = hidden_recording_events(messages).into_iter().collect();
        found.sort_unstable();
        found
    }

    #[test]
    fn a_link_less_event_hides_only_when_a_link_follows() {
        let plain = message(false, false);
        // Both the early events go, the finished one and the text stay.
        assert_eq!(
            hidden(&[
                message(true, false),
                plain.clone(),
                message(true, false),
                message(true, true),
                plain.clone(),
            ]),
            vec![0, 2]
        );
        // With no link after it, the event is the only sign and stays.
        assert!(hidden(&[message(true, true), message(true, false)]).is_empty());
        assert!(hidden(&[message(true, false), plain]).is_empty());
        assert!(hidden(&[]).is_empty());
    }
}
