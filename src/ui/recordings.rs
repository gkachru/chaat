//! Recordings: the meeting recordings Chaat has seen in meeting chats it
//! loaded, newest first. A row selects a recording and the main pane shows
//! it as a card, with the buttons that open it, its meeting chat or copy its
//! link. The list is a local cache query, see `AccountCommand::Recordings`.

use egui::{Align, Layout, Sense, vec2};

use super::widgets;
use crate::app::App;
use crate::model::{Action, ConversationId, Millis};
use crate::theme::{self, Icon};

/// The tile in a list row, and the one on the card.
const ROW_TILE: f32 = 40.0;
const CARD_TILE: f32 = 64.0;
/// The widest the card's column gets.
const CARD_WIDTH: f32 = 440.0;

/// What a recording is, for drawing: the message's recording with its time
/// and the chat it came from.
struct Entry {
    name: String,
    url: String,
    duration: Option<String>,
    created: Millis,
    chat: Option<ConversationId>,
    /// The meeting chat's title, when it is a chat in the cache.
    chat_title: Option<String>,
}

impl Entry {
    /// "Tuesday 24 November 00:43 · 1h 9m", as a call line says the time.
    fn when(&self, now: Millis) -> String {
        let mut text = crate::time::at_label(self.created, now);
        if let Some(duration) = &self.duration {
            text.push_str(" · ");
            text.push_str(duration);
        }
        text
    }
}

fn entries(app: &App) -> Vec<Entry> {
    let me = app.me().to_owned();
    app.recordings
        .iter()
        .filter(|message| !message.deleted)
        .filter_map(|message| {
            let recording = message.recording.as_ref()?;
            let chat_title = match &message.conversation {
                Some(ConversationId::Chat(id)) => app.chat(id).map(|c| c.title(&me)),
                _ => None,
            };
            Some(Entry {
                name: recording.name.clone(),
                url: recording.url.clone(),
                duration: recording.duration.clone(),
                created: message.created,
                chat: message.conversation.clone(),
                chat_title,
            })
        })
        .collect()
}

/// The index of the recording to show: the one `selected` names, else the
/// newest, by time.
fn selected_index(entries: &[Entry], selected: Option<&str>) -> Option<usize> {
    selected
        .and_then(|url| entries.iter().position(|e| e.url == url))
        .or_else(|| {
            entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| std::cmp::Reverse(e.created))
                .map(|(index, _)| index)
        })
}

/// The sidebar list.
pub fn list(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let entries = entries(app);
    let Some(active) = selected_index(&entries, app.selected_recording.as_deref()) else {
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
    };
    // A missing or stale selection becomes the newest.
    if app.selected_recording.as_deref() != Some(entries[active].url.as_str()) {
        app.selected_recording = Some(entries[active].url.clone());
    }
    let now = crate::time::now();
    let mut select = None;
    egui::ScrollArea::vertical()
        .id_salt("recordings-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, entry) in entries.iter().enumerate() {
                let selected = index == active;
                let (row, inner) = widgets::list_row(ui, &palette, 56.0, selected, Sense::click());
                // Text is painted, not laid out as labels: a label would sit
                // over the row and take its hover and click.
                let tile = egui::Rect::from_min_size(
                    egui::pos2(inner.left() + 8.0, inner.center().y - ROW_TILE / 2.0),
                    egui::Vec2::splat(ROW_TILE),
                );
                let left = tile.right() + 10.0;
                let text_width = (inner.right() - 8.0 - left).max(40.0);
                let mut detail = entry.chat_title.clone().unwrap_or_default();
                if !detail.is_empty() {
                    detail.push_str(" · ");
                }
                detail.push_str(&entry.when(now));
                let title = widgets::elided(
                    ui,
                    &entry.name,
                    theme::semibold(14.0),
                    palette.text,
                    text_width,
                );
                let (_, dim) = palette.row_text(selected);
                let meta = widgets::elided(ui, &detail, theme::regular(12.0), dim, text_width);
                if ui.is_rect_visible(row.rect) {
                    widgets::glass_tile(ui.painter(), &palette, tile, row.hovered());
                    Icon::Play.image(palette.accent, 20.0).paint_at(
                        ui,
                        egui::Rect::from_center_size(tile.center(), egui::Vec2::splat(20.0)),
                    );
                    let block = title.size().y + meta.size().y;
                    let top = inner.center().y - block / 2.0;
                    let meta_top = top + title.size().y;
                    ui.painter()
                        .galley(egui::pos2(left, top), title, palette.text);
                    ui.painter().galley(egui::pos2(left, meta_top), meta, dim);
                }
                let row = row.on_hover_cursor(egui::CursorIcon::PointingHand);
                if row.clicked() {
                    select = Some(entry.url.clone());
                }
            }
        });
    if select.is_some() {
        app.selected_recording = select;
    }
}

/// The main pane: the selected recording as a card, or what the section is.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let header = egui::Rect::from_min_size(
        ui.max_rect().min,
        vec2(ui.max_rect().width(), theme::HEADER_HEIGHT),
    );
    widgets::window_drag(ui, header, "recordings-drag");
    let entries = entries(app);
    let Some(index) = selected_index(&entries, app.selected_recording.as_deref()) else {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Play,
            "Meeting recordings",
            "Recordings open in your browser, in Microsoft Teams' own player. Chaat lists only those from meeting chats it has loaded.",
        );
        return;
    };
    let entry = &entries[index];
    let now = crate::time::now();
    let pane = ui.max_rect();
    let width = (pane.width() - 48.0).clamp(120.0, CARD_WIDTH);
    let top = pane.top() + theme::HEADER_HEIGHT + 32.0;
    let column = egui::Rect::from_min_size(
        egui::pos2((pane.center().x - width / 2.0).max(pane.left()), top),
        vec2(width, (pane.bottom() - top).max(0.0)),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(column)
            .layout(Layout::top_down(Align::Min)),
    );
    let ui = &mut child;
    ui.set_width(width);

    let (tile, _) = ui.allocate_exact_size(egui::Vec2::splat(CARD_TILE), Sense::hover());
    widgets::glass_tile(ui.painter(), &palette, tile, false);
    Icon::Play.image(palette.accent, 28.0).paint_at(
        ui,
        egui::Rect::from_center_size(tile.center(), egui::Vec2::splat(28.0)),
    );
    ui.add_space(14.0);
    ui.label(
        egui::RichText::new(&entry.name)
            .font(theme::semibold(20.0))
            .color(palette.text),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(entry.when(now))
            .font(theme::regular(13.0))
            .color(palette.dim),
    );
    if let Some(title) = &entry.chat_title {
        ui.label(
            egui::RichText::new(format!("in {title}"))
                .font(theme::regular(13.0))
                .color(palette.dim),
        );
    }
    ui.add_space(16.0);

    let mut actions = Vec::new();
    let open = widgets::pill(ui, &palette, Icon::Play, "Open recording", true)
        .on_hover_text("Open the recording in your browser");
    widgets::on_click(&open, &mut actions, Action::OpenUrl(entry.url.clone()));
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if let Some(chat) = &entry.chat {
            let button = widgets::pill(ui, &palette, Icon::Chat, "Open meeting chat", false);
            widgets::on_click(
                &button,
                &mut actions,
                Action::OpenConversation(chat.clone()),
            );
        }
        let copy = widgets::pill(ui, &palette, Icon::Copy, "Copy link", false);
        widgets::on_click(&copy, &mut actions, Action::CopyText(entry.url.clone()));
    });

    // The others from the same chat.
    let others: Vec<&Entry> = entries
        .iter()
        .enumerate()
        .filter(|(i, e)| *i != index && entry.chat.is_some() && e.chat == entry.chat)
        .map(|(_, e)| e)
        .collect();
    let mut select = None;
    if !others.is_empty() {
        ui.add_space(24.0);
        ui.label(
            egui::RichText::new("Also from this chat")
                .font(theme::regular(12.0))
                .color(palette.dim),
        );
        ui.add_space(4.0);
        for other in others {
            let (row, inner) = widgets::list_row(ui, &palette, 32.0, false, Sense::click());
            if ui.is_rect_visible(row.rect) {
                let icon = egui::Rect::from_center_size(
                    egui::pos2(inner.left() + 14.0, inner.center().y),
                    egui::Vec2::splat(14.0),
                );
                Icon::Play.image(palette.accent, 14.0).paint_at(ui, icon);
                let left = icon.right() + 8.0;
                let text = format!("{} · {}", other.name, other.when(now));
                let galley = widgets::elided(
                    ui,
                    &text,
                    theme::regular(13.0),
                    palette.text,
                    (inner.right() - 8.0 - left).max(20.0),
                );
                let y = inner.center().y - galley.size().y / 2.0;
                ui.painter()
                    .galley(egui::pos2(left, y), galley, palette.text);
            }
            let row = row.on_hover_cursor(egui::CursorIcon::PointingHand);
            if row.clicked() {
                select = Some(other.url.clone());
            }
        }
    }
    if select.is_some() {
        app.selected_recording = select;
    }
    app.actions.extend(actions);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, created: Millis) -> Entry {
        Entry {
            name: url.into(),
            url: url.into(),
            duration: None,
            created,
            chat: None,
            chat_title: None,
        }
    }

    #[test]
    fn the_selection_falls_back_to_the_newest() {
        let entries = [entry("a", 10), entry("b", 30), entry("c", 20)];
        assert_eq!(selected_index(&entries, None), Some(1));
        // A recording that is gone from the list.
        assert_eq!(selected_index(&entries, Some("gone")), Some(1));
        // A recording still there stays selected, newest or not.
        assert_eq!(selected_index(&entries, Some("c")), Some(2));
        assert_eq!(selected_index(&entries, Some("a")), Some(0));
        assert_eq!(selected_index(&[], Some("a")), None);
    }
}
