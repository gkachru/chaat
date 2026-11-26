//! Recordings: the meeting recordings Chaat has seen in meeting chats it
//! loaded, newest first. A row opens the recording in the browser; "Chat"
//! opens the meeting chat. The list is a local cache query, see
//! `AccountCommand::Recordings`.

use egui::{Sense, vec2};

use super::widgets;
use crate::app::App;
use crate::model::{Action, ConversationId};
use crate::theme::{self, Icon};

/// The sidebar list.
pub fn list(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    if app.recordings.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "No recordings yet. Recordings appear here after you open a meeting chat that has one.",
                    )
                    .color(palette.dim),
                )
                .wrap(),
            );
        });
        return;
    }
    let me = app.me().to_owned();
    let now = crate::time::now();
    let rows: Vec<(String, String, String, Option<ConversationId>)> = app
        .recordings
        .iter()
        .filter_map(|message| {
            let recording = message.recording.as_ref()?;
            let chat = match &message.conversation {
                Some(ConversationId::Chat(id)) => app.chat(id).map(|c| c.title(&me)),
                _ => None,
            };
            let mut detail = chat.unwrap_or_default();
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str(&crate::time::day_label(message.created, now));
            if let Some(duration) = &recording.duration {
                detail.push_str(" · ");
                detail.push_str(duration);
            }
            Some((
                recording.name.clone(),
                detail,
                recording.url.clone(),
                message.conversation.clone(),
            ))
        })
        .collect();
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("recordings-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (name, detail, url, chat) in &rows {
                let (row, inner) = widgets::list_row(ui, &palette, 56.0, false, Sense::click());
                // Text is painted, not laid out as labels: a label would sit
                // over the row and take its hover and click.
                let pill = if chat.is_some() {
                    widgets::pill_width(ui, "Chat") + 12.0
                } else {
                    0.0
                };
                let left = inner.left() + 16.0;
                let text_width = (inner.right() - 8.0 - pill - left).max(40.0);
                let name_galley =
                    widgets::elided(ui, name, theme::medium(13.0), palette.text, text_width);
                let detail_galley =
                    widgets::elided(ui, detail, theme::regular(12.0), palette.dim, text_width);
                let block = name_galley.size().y + detail_galley.size().y;
                let top = inner.center().y - block / 2.0;
                let detail_top = top + name_galley.size().y;
                ui.painter()
                    .galley(egui::pos2(left, top), name_galley, palette.text);
                ui.painter()
                    .galley(egui::pos2(left, detail_top), detail_galley, palette.dim);
                row.clone()
                    .on_hover_text("Open the recording in your browser");
                widgets::on_click(&row, &mut actions, Action::OpenUrl(url.clone()));
                if let Some(chat) = chat {
                    let width = pill - 12.0;
                    let rect = egui::Rect::from_center_size(
                        egui::pos2(inner.right() - 8.0 - width / 2.0, inner.center().y),
                        vec2(width, 28.0),
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    let button = widgets::pill(&mut child, &palette, Icon::Chat, "Chat", false)
                        .on_hover_text("Open the meeting chat");
                    widgets::on_click(
                        &button,
                        &mut actions,
                        Action::OpenConversation(chat.clone()),
                    );
                }
            }
        });
    app.actions.extend(actions);
}

/// The main pane: what the section is.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let header = egui::Rect::from_min_size(
        ui.max_rect().min,
        vec2(ui.max_rect().width(), theme::HEADER_HEIGHT),
    );
    widgets::window_drag(ui, header, "recordings-drag");
    widgets::empty_state(
        ui,
        &palette,
        Icon::Play,
        "Meeting recordings",
        "Recordings open in your browser, in Microsoft Teams' own player. Chaat lists only those from meeting chats it has loaded.",
    );
}
