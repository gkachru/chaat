//! The emoji picker popup, shared by the composer and reactions: a search
//! field, group tabs and a grid. The caller owns the popup (where it opens,
//! when it closes); this draws its contents and says which emoji was chosen.

use std::sync::{Arc, OnceLock};

use egui::{CornerRadius, Frame, Id, Margin, Sense, Ui, Vec2, vec2};
use emojis::Emoji;

use crate::emoji;
use crate::theme::{self, Palette};

/// The popup's size in points, margins included.
pub const SIZE: Vec2 = vec2(336.0, 320.0);
const MARGIN: i8 = 8;
const COLUMNS: usize = 8;
const CELL: Vec2 = vec2(36.0, 32.0);
const TAB_HEIGHT: f32 = 28.0;
/// Most search hits shown.
const SEARCH_LIMIT: usize = 240;

type Groups = Vec<(&'static str, Vec<&'static Emoji>)>;

fn groups() -> &'static Groups {
    static GROUPS: OnceLock<Groups> = OnceLock::new();
    GROUPS.get_or_init(emoji::groups)
}

/// Search hits for `query`, kept in egui temp data under `id` so an open
/// popup does not rescan the table every frame.
fn cached_search(ui: &Ui, id: Id, query: &str) -> Arc<Vec<&'static Emoji>> {
    let key = id.with("hits");
    let cached = ui.data(|d| d.get_temp::<(String, Arc<Vec<&'static Emoji>>)>(key));
    match cached {
        Some((cached_query, hits)) if cached_query == query => hits,
        _ => {
            let hits = Arc::new(emoji::search(query, SEARCH_LIMIT));
            ui.data_mut(|d| d.insert_temp(key, (query.to_owned(), Arc::clone(&hits))));
            hits
        }
    }
}

/// What a cell says on hover: the name and its first shortcode, as in
/// "grinning face  :grinning:".
fn hover_label(emoji: &Emoji) -> String {
    match emoji.shortcode() {
        Some(code) => format!("{}  :{code}:", emoji.name()),
        None => emoji.name().to_owned(),
    }
}

/// Id of the grid cell for an emoji, under the picker's `id`.
fn cell_id(id: Id, emoji: &Emoji) -> Id {
    id.with(("cell", emoji.as_str()))
}

/// Forgets the search text and group of the picker under `id`, for when its
/// popup closed without a pick: it reopens fresh, with the search focused.
pub fn reset(ctx: &egui::Context, id: Id) {
    ctx.data_mut(|d| {
        d.remove_temp::<String>(id);
        d.remove_temp::<usize>(id.with("group"));
    });
}

/// Draws the picker and returns the emoji picked this frame, if any.
///
/// The search text and the open group live in egui's temp data under `id`
/// (one picker per id) and are cleared when something is picked.
pub fn show(ui: &mut Ui, palette: &Palette, id: Id) -> Option<String> {
    let stored = ui.data(|d| d.get_temp::<String>(id));
    let first_frame = stored.is_none();
    let mut query = stored.unwrap_or_default();
    let group_id = id.with("group");
    let mut group = ui.data(|d| d.get_temp::<usize>(group_id)).unwrap_or(0);
    let mut picked: Option<&'static Emoji> = None;

    Frame::new()
        .inner_margin(Margin::same(MARGIN))
        .show(ui, |ui| {
            let inner = SIZE - Vec2::splat(f32::from(MARGIN) * 2.0);
            ui.set_min_size(inner);
            ui.set_max_size(inner);
            ui.spacing_mut().item_spacing = vec2(0.0, 6.0);

            let edit =
                crate::ui::widgets::text_field(&mut query, "Search emoji", theme::regular(13.0))
                    .id(id.with("search"));
            let search = ui.add(edit);
            if first_frame {
                search.request_focus();
            }
            let mut reset_scroll = search.changed();
            let searching = !query.trim().is_empty();

            if !searching {
                let groups = groups();
                group = group.min(groups.len().saturating_sub(1));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let width = inner.x / groups.len() as f32;
                    for (index, (name, emoji)) in groups.iter().enumerate() {
                        let Some(first) = emoji.first() else { continue };
                        let (_, rect) = ui.allocate_space(vec2(width, TAB_HEIGHT));
                        let response = ui
                            .interact(rect, id.with(("tab", index)), Sense::click())
                            .on_hover_text(*name)
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        let fill = if index == group {
                            Some(palette.surface_active)
                        } else if response.hovered() {
                            Some(palette.surface_hover)
                        } else {
                            None
                        };
                        if let Some(fill) = fill {
                            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
                        }
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            first.as_str(),
                            theme::regular(20.0),
                            palette.text,
                        );
                        if response.clicked() {
                            group = index;
                            reset_scroll = true;
                        }
                    }
                });
            }

            let hits: Arc<Vec<&'static Emoji>>;
            let shown: &[&'static Emoji] = if searching {
                hits = cached_search(ui, id, &query);
                &hits
            } else {
                &groups()[group].1
            };
            if shown.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("No emoji found")
                        .font(theme::regular(13.0))
                        .color(palette.dim),
                );
                return;
            }
            let rows = shown.len().div_ceil(COLUMNS);
            let mut area = egui::ScrollArea::vertical()
                .id_salt(id.with(("scroll", searching, group)))
                .auto_shrink([false, false]);
            if reset_scroll {
                area = area.vertical_scroll_offset(0.0);
            }
            area.show_rows(ui, CELL.y, rows, |ui, range| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let visible = &shown[range.start * COLUMNS..(range.end * COLUMNS).min(shown.len())];
                fastframe_emoji::prewarm(ui.ctx(), visible.iter().map(|e| e.as_str()));
                for row in range {
                    ui.horizontal(|ui| {
                        let from = row * COLUMNS;
                        for emoji in &shown[from..(from + COLUMNS).min(shown.len())] {
                            let (_, rect) = ui.allocate_space(CELL);
                            let response = ui
                                .interact(rect, cell_id(id, emoji), Sense::click())
                                .on_hover_text(hover_label(emoji))
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if response.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    CornerRadius::same(6),
                                    palette.surface_hover,
                                );
                            }
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                emoji.as_str(),
                                theme::regular(20.0),
                                palette.text,
                            );
                            if response.clicked() {
                                picked = Some(emoji);
                            }
                        }
                    });
                }
            });
        });

    if let Some(emoji) = picked {
        ui.data_mut(|d| {
            d.remove_temp::<String>(id);
            d.remove_temp::<usize>(group_id);
        });
        return Some(emoji.as_str().to_owned());
    }
    ui.data_mut(|d| {
        d.insert_temp(id, query);
        d.insert_temp(group_id, group);
    });
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, PointerButton, Pos2, RawInput, Rect};

    struct Frames {
        ctx: Context,
        palette: Palette,
        time: f64,
        id: Id,
    }

    impl Frames {
        fn new() -> Self {
            let ctx = Context::default();
            crate::ui::install(&ctx, true);
            Self {
                ctx,
                palette: Palette::dark(),
                time: 0.0,
                id: Id::new("picker-test"),
            }
        }

        /// Runs one frame; returns the pick and the picker's rect.
        fn frame(&mut self, events: Vec<Event>) -> (Option<String>, Rect) {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let (palette, id) = (&self.palette, self.id);
            let mut out = (None, Rect::NOTHING);
            let mut output = self.ctx.run_ui(input, |ui| {
                let response = ui.scope(|ui| show(ui, palette, id));
                out = (response.inner, response.response.rect);
            });
            // No renderer here: drop the texture uploads on purpose.
            output.textures_delta.clear();
            out
        }

        fn cell(&self, emoji: &Emoji) -> Option<Rect> {
            self.ctx
                .read_response(cell_id(self.id, emoji))
                .map(|r| r.rect)
        }

        fn query(&self) -> Option<String> {
            self.ctx.data(|d| d.get_temp::<String>(self.id))
        }
    }

    #[test]
    fn reset_clears_search_and_group() {
        let mut f = Frames::new();
        f.frame(Vec::new());
        assert_eq!(f.query(), Some(String::new()));
        reset(&f.ctx, f.id);
        assert_eq!(f.query(), None);
    }

    #[test]
    fn pick_returns_emoji() {
        let mut f = Frames::new();
        f.frame(Vec::new());
        f.frame(Vec::new());
        let first = groups()[0].1[0];
        let center = f.cell(first).expect("first cell is drawn").center();
        f.frame(vec![Event::PointerMoved(center)]);
        let press = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        f.frame(vec![press(true)]);
        let (picked, _) = f.frame(vec![press(false)]);
        assert_eq!(picked.as_deref(), Some(first.as_str()));
        // The search text is cleared on pick.
        assert_eq!(f.query(), None);
    }

    #[test]
    fn search_filters() {
        let mut f = Frames::new();
        f.frame(Vec::new());
        f.ctx.data_mut(|d| d.insert_temp(f.id, "heart".to_owned()));
        f.frame(Vec::new());
        f.frame(Vec::new());
        let hits = emoji::search("heart", SEARCH_LIMIT);
        assert!(!hits.is_empty());
        assert!(f.cell(hits[0]).is_some(), "best match is shown");
        let grin = emojis::get("\u{1F600}").unwrap();
        assert!(!hits.contains(&grin));
        assert!(f.cell(grin).is_none(), "a non-match is hidden");
    }

    #[test]
    fn hover_label_names_the_emoji_and_its_shortcode() {
        let grin = emojis::get("\u{1F600}").unwrap();
        assert_eq!(hover_label(grin), "grinning face  :grinning:");
        const { assert!(SIZE.x < 360.0 && SIZE.y < 340.0) };
    }

    #[test]
    fn search_is_cached_per_query() {
        let f = Frames::new();
        let mut first = None;
        let mut second = None;
        let mut third = None;
        let run = |query: &str, slot: &mut Option<Arc<Vec<&'static Emoji>>>| {
            let mut out = f.ctx.run_ui(RawInput::default(), |ui| {
                *slot = Some(cached_search(ui, f.id, query));
            });
            out.textures_delta.clear();
        };
        run("heart", &mut first);
        run("heart", &mut second);
        run("cat", &mut third);
        assert!(Arc::ptr_eq(
            first.as_ref().unwrap(),
            second.as_ref().unwrap()
        ));
        assert!(!Arc::ptr_eq(
            first.as_ref().unwrap(),
            third.as_ref().unwrap()
        ));
    }

    #[test]
    fn query_kept_per_id() {
        let mut f = Frames::new();
        f.ctx.data_mut(|d| d.insert_temp(f.id, "cat".to_owned()));
        f.frame(Vec::new());
        assert_eq!(f.query().as_deref(), Some("cat"));
        // Another picker has its own text.
        let other = Id::new("other-picker");
        assert_eq!(f.ctx.data(|d| d.get_temp::<String>(other)), None);
    }

    #[test]
    fn no_match_says_so() {
        let mut f = Frames::new();
        f.ctx
            .data_mut(|d| d.insert_temp(f.id, "zzzzqqqq".to_owned()));
        let (picked, _) = f.frame(Vec::new());
        assert_eq!(picked, None);
        assert!(f.cell(groups()[0].1[0]).is_none());
    }

    #[test]
    fn fixed_size() {
        let mut f = Frames::new();
        let (_, rect) = f.frame(Vec::new());
        assert!(
            (rect.size() - SIZE).abs().max_elem() < 0.5,
            "{:?}",
            rect.size()
        );
        // Searching does not resize it.
        f.ctx.data_mut(|d| d.insert_temp(f.id, "a".to_owned()));
        let (_, rect) = f.frame(Vec::new());
        assert!(
            (rect.size() - SIZE).abs().max_elem() < 0.5,
            "{:?}",
            rect.size()
        );
        f.ctx
            .data_mut(|d| d.insert_temp(f.id, "zzzzqqqq".to_owned()));
        let (_, rect) = f.frame(Vec::new());
        assert!(
            (rect.size() - SIZE).abs().max_elem() < 0.5,
            "{:?}",
            rect.size()
        );
    }
}
