//! Find in the open conversation (the bar under the header) and the Jump to
//! latest button over the history.

use egui::{CornerRadius, Frame, Id, Key, Margin, Modifiers, Sense, Stroke, Ui, vec2};

use crate::app::App;
use crate::model::{Action, ConversationId};
use crate::theme::{self, Icon, Palette};
use crate::ui::widgets;

/// Set by the shortcut: the bar takes the keyboard on the next frame.
pub fn focus_flag() -> Id {
    Id::new("chaat-find-focus")
}

/// What the bar says about its results: nothing for a query too short to
/// search, "Searching…" until the hits answer it, then the position.
pub fn count_label(query: &str, searched: &str, hits: usize, current: usize) -> String {
    if query.trim().chars().count() < 2 {
        String::new()
    } else if searched != query {
        "Searching…".to_owned()
    } else if hits == 0 {
        "No matches".to_owned()
    } else {
        format!("{} of {hits}", current.min(hits - 1) + 1)
    }
}

/// Whether the history is scrolled up more than a screen from its end.
pub fn jump_visible(content_height: f32, offset: f32, view_height: f32) -> bool {
    content_height - (offset + view_height) > view_height
}

/// The widest the query field gets, in points.
const FIELD_WIDTH: f32 = 320.0;

fn query_key() -> Id {
    Id::new("chaat-find-active-query")
}

/// The text to mark in message bodies while the find bar is open with a
/// query of two or more characters.
pub fn active_query(ctx: &egui::Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<String>(query_key()))
}

/// Byte ranges of the case-insensitive occurrences of `query` in `text`.
/// Ranges sit on char boundaries and do not overlap. An empty query has none.
pub fn matches(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let wanted: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let mut matched = 0;
        let mut next = at;
        // Whole chars only: a char whose lowercase is only partly wanted
        // does not match.
        while next < chars.len() && matched < wanted.len() {
            let lower: Vec<char> = chars[next].1.to_lowercase().collect();
            if wanted[matched..].starts_with(&lower) {
                matched += lower.len();
                next += 1;
            } else {
                break;
            }
        }
        if matched == wanted.len() {
            let end = chars.get(next).map_or(text.len(), |(i, _)| *i);
            found.push(chars[at].0..end);
            at = next;
        } else {
            at += 1;
        }
    }
    found
}

/// The 2 px outline around the bubble of the hit the bar is on.
pub fn outline_current(ui: &Ui, app: &App, message_id: &str, bubble: egui::Rect) {
    let Some(find) = &app.find else {
        return;
    };
    if find
        .hits
        .get(find.current)
        .is_some_and(|h| h.message_id == message_id)
    {
        ui.painter().rect_stroke(
            bubble.expand(1.0),
            CornerRadius::same(7),
            Stroke::new(2.0, app.palette.accent),
            egui::StrokeKind::Outside,
        );
    }
}

/// An up or down button that does nothing, dimmed and without a hover, when
/// there are no hits.
fn nav_button(
    ui: &mut Ui,
    palette: &Palette,
    icon: Icon,
    tooltip: &str,
    enabled: bool,
) -> egui::Response {
    if enabled {
        return widgets::icon_button_sized(
            ui,
            palette,
            icon,
            tooltip,
            28.0,
            16.0,
            palette.secondary,
        );
    }
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        icon.image(palette.dim.gamma_multiply(0.6), 16.0).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)),
        );
    }
    response
}

/// The find bar, shown while `App::find` is open. Enter goes to the next
/// (older) hit, Shift+Enter to the previous one, Esc closes.
pub fn show(app: &mut App, ui: &mut Ui, conversation: &ConversationId) {
    let Some(find) = &app.find else {
        ui.ctx().data_mut(|d| d.remove::<String>(query_key()));
        return;
    };
    let palette = app.palette;
    let (searched, hits, current) = (find.searched.clone(), find.hits.len(), find.current);
    let mut query = find.query.clone();
    let mut actions: Vec<Action> = Vec::new();

    // A bar left open in another conversation closes.
    let owner_id = Id::new("chaat-find-owner");
    let owner: Option<String> = ui.ctx().data(|d| d.get_temp(owner_id));
    match owner {
        Some(key) if key != conversation.key() => {
            ui.ctx().data_mut(|d| d.remove::<String>(owner_id));
            app.actions.push(Action::Find(None));
            return;
        }
        Some(_) => {}
        None => ui.ctx().data_mut(|d| {
            d.insert_temp(owner_id, conversation.key());
        }),
    }

    let field_id = Id::new("chaat-find-field");
    // Esc surrenders egui's focus before the frame, so look at last frame too.
    let has_focus = ui.memory(|m| m.has_focus(field_id) || m.had_focus_last_frame(field_id));
    if has_focus {
        ui.input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::Escape) {
                actions.push(Action::Find(None));
            } else if input.consume_key(Modifiers::SHIFT, Key::Enter) {
                actions.push(Action::FindNext(-1));
            } else if input.consume_key(Modifiers::NONE, Key::Enter) {
                actions.push(Action::FindNext(1));
            }
        });
    }
    egui::Panel::top("conversation-find")
        .exact_size(40.0)
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(palette.window)
                .inner_margin(Margin::symmetric(16, 4)),
        )
        .show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter().hline(
                rect.x_range().expand(16.0),
                rect.bottom(),
                Stroke::new(1.0, palette.outline),
            );
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                theme::icon(ui, Icon::Search, 16.0, palette.dim);
                let label = count_label(&query, &searched, hits, current);
                // Room for the count and the three buttons on the right.
                let controls = 96.0 + 3.0 * 28.0 + 3.0 * 6.0;
                let width = FIELD_WIDTH.min((ui.available_width() - controls).max(80.0));
                let edit = widgets::text_field(
                    &mut query,
                    "Find in this conversation",
                    theme::regular(13.0),
                )
                .id(field_id)
                .desired_width(width);
                let response = ui.add(edit);
                if ui
                    .ctx()
                    .data_mut(|d| d.remove_temp::<bool>(focus_flag()))
                    .is_some()
                {
                    response.request_focus();
                    // Select the old text so typing replaces it.
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field_id) {
                        let end = query.chars().count();
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(end),
                            )));
                        state.store(ui.ctx(), field_id);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let close = widgets::icon_button_sized(
                        ui,
                        &palette,
                        Icon::Close,
                        "Close (Esc)",
                        28.0,
                        16.0,
                        palette.secondary,
                    );
                    widgets::on_click(&close, &mut actions, Action::Find(None));
                    let any = hits > 0;
                    let newer =
                        nav_button(ui, &palette, Icon::ChevronDown, "Next match (Enter)", any);
                    widgets::on_click(&newer, &mut actions, Action::FindNext(1));
                    let older = nav_button(
                        ui,
                        &palette,
                        Icon::ChevronUp,
                        "Previous match (Shift+Enter)",
                        any,
                    );
                    widgets::on_click(&older, &mut actions, Action::FindNext(-1));
                    ui.label(
                        egui::RichText::new(label)
                            .font(theme::regular(12.0))
                            .color(palette.secondary),
                    );
                });
            });
        });
    if actions.iter().any(|a| matches!(a, Action::Find(None))) {
        ui.ctx().data_mut(|d| {
            d.remove::<String>(owner_id);
            d.remove::<String>(query_key());
        });
        // Closing hands the keyboard back to the composer.
        app.focus_composer = true;
    } else if query.trim().chars().count() >= 2 {
        let shared = query.clone();
        ui.ctx().data_mut(|d| d.insert_temp(query_key(), shared));
    } else {
        ui.ctx().data_mut(|d| d.remove::<String>(query_key()));
    }
    if query
        != app
            .find
            .as_ref()
            .map(|f| f.query.clone())
            .unwrap_or_default()
    {
        actions.push(Action::Find(Some(query)));
    }
    app.actions.extend(actions);
}

/// The round "Jump to latest" button, floating at the bottom right of the
/// history's `view` when it is scrolled well up. Pushes `JumpToLatest`.
pub fn jump_button(
    ui: &Ui,
    palette: &Palette,
    conversation: &ConversationId,
    view: egui::Rect,
    visible: bool,
    actions: &mut Vec<Action>,
) {
    if !visible {
        return;
    }
    const SIZE: f32 = 36.0;
    egui::Area::new(Id::new(("chaat-jump-latest", conversation.key())))
        .order(egui::Order::Foreground)
        .fixed_pos(view.right_bottom() - vec2(SIZE + 20.0, SIZE + 16.0))
        .show(ui.ctx(), |ui| {
            let (rect, response) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::click());
            let response = response
                .on_hover_text("Jump to latest")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let fill = if response.hovered() {
                palette.surface_hover
            } else {
                palette.surface
            };
            ui.painter().add(
                egui::Shadow {
                    offset: [0, 3],
                    blur: 10,
                    spread: 0,
                    color: palette.shadow,
                }
                .as_shape(rect, CornerRadius::same(18)),
            );
            ui.painter().circle_filled(rect.center(), SIZE / 2.0, fill);
            ui.painter().circle_stroke(
                rect.center(),
                SIZE / 2.0,
                Stroke::new(1.0, palette.outline),
            );
            Icon::ChevronDown.image(palette.text, 18.0).paint_at(
                ui,
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(18.0)),
            );
            widgets::on_click(&response, actions, Action::JumpToLatest);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_label_says_where_you_are() {
        assert_eq!(count_label("", "", 0, 0), "");
        assert_eq!(count_label("a", "a", 0, 0), "");
        assert_eq!(count_label("lunch", "lun", 3, 0), "Searching…");
        assert_eq!(count_label("lunch", "lunch", 0, 0), "No matches");
        assert_eq!(count_label("lunch", "lunch", 4, 0), "1 of 4");
        assert_eq!(count_label("lunch", "lunch", 4, 3), "4 of 4");
        // A stale index never reads past the end.
        assert_eq!(count_label("lunch", "lunch", 2, 9), "2 of 2");
    }

    #[test]
    fn matches_ignores_case_and_returns_byte_ranges() {
        assert_eq!(matches("Lunch at lunch", "LUNCH"), vec![0..5, 9..14]);
        assert_eq!(matches("abc", ""), Vec::<std::ops::Range<usize>>::new());
        assert_eq!(matches("abc", "d"), Vec::<std::ops::Range<usize>>::new());
    }

    #[test]
    fn matches_is_char_boundary_safe() {
        // Multibyte text before and inside the match.
        let text = "é✓ Café ✓";
        let hits = matches(text, "CAFÉ");
        assert_eq!(hits.len(), 1);
        assert_eq!(&text[hits[0].clone()], "Café");
        for range in matches("日本語 日本", "日本") {
            assert!(("日本語 日本").is_char_boundary(range.start));
            assert!(("日本語 日本").is_char_boundary(range.end));
        }
        // Matches do not overlap.
        assert_eq!(matches("aaaa", "aa"), vec![0..2, 2..4]);
        // A query longer than the text has no match.
        assert!(matches("a", "abc").is_empty());
    }

    #[test]
    fn jump_shows_after_more_than_a_screen() {
        // A 500 high view over 3000 of content.
        assert!(!jump_visible(3000.0, 2500.0, 500.0), "at the end");
        assert!(
            !jump_visible(3000.0, 2000.0, 500.0),
            "exactly one screen up"
        );
        assert!(
            jump_visible(3000.0, 1900.0, 500.0),
            "more than one screen up"
        );
        assert!(!jump_visible(400.0, 0.0, 500.0), "short history");
    }
}
