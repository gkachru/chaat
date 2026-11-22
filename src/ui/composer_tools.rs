//! What the composer adds around its text editor: the format bar and
//! shortcuts, live markdown styling, the emoji picker and typed emoji
//! conversion. `conversation.rs` calls these; the text edits themselves are
//! pure functions over char indices (egui's `CCursor`).

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::{Event, Id, Popup, PopupCloseBehavior, Ui};

use crate::compose_format::{self, Format};
use crate::theme::{self, Icon, Palette};
use crate::ui::{emoji_picker, widgets};

/// The Ctrl/Cmd shortcut at the front of `events`, if any: the key event is
/// removed so no other widget sees it.
pub fn take_shortcut(events: &mut Vec<Event>) -> Option<Format> {
    let at = events.iter().position(|event| {
        matches!(event, Event::Key { key, pressed: true, modifiers, .. }
            if compose_format::shortcut(*modifiers, *key).is_some())
    })?;
    let Event::Key { key, modifiers, .. } = events.remove(at) else {
        return None;
    };
    compose_format::shortcut(modifiers, key)
}

/// `text` with `range` (chars) replaced by `insert`, and the caret after it.
pub fn replace_range(text: &str, range: Range<usize>, insert: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let end = range.end.min(chars.len());
    let start = range.start.min(end);
    let mut out: String = chars[..start].iter().collect();
    out.push_str(insert);
    out.extend(&chars[end..]);
    (out, start + insert.chars().count())
}

/// The replacement for a `:shortcode:` just typed, or an emoticon followed
/// by a space, ending at `cursor`: the new text and the caret after it.
/// An emoticon still being typed (no space yet) is left alone.
pub fn convert_typed(text: &str, cursor: usize) -> Option<(String, usize)> {
    let (range, emoji) = crate::emoji::convert_before_cursor(text, cursor)?;
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let shortcode = chars[range.start] == ':' && chars[range.end - 1] == ':' && range.len() > 2;
    if !shortcode && chars.get(cursor.wrapping_sub(1)) != Some(&' ') {
        return None;
    }
    let (out, _) = replace_range(text, range.clone(), emoji);
    Some((out, cursor - range.len() + emoji.chars().count()))
}

/// The editor's selection as ordered char indices, or the end of `text`.
pub fn selection(ctx: &egui::Context, id: Id, text: &str) -> Range<usize> {
    egui::TextEdit::load_state(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .map(|r| r.primary.index.min(r.secondary.index).0..r.primary.index.max(r.secondary.index).0)
        .unwrap_or_else(|| {
            let end = text.chars().count();
            end..end
        })
}

/// Sets the editor's selection (chars) before it draws.
pub fn set_selection(ctx: &egui::Context, id: Id, range: Range<usize>) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(range.start),
        CCursor::new(range.end),
    )));
    state.store(ctx, id);
}

/// Makes `text` with `selection` an undo point of the editor, so Ctrl+Z
/// after an edit made outside the editor (format, emoji, conversion) goes
/// back to exactly this text instead of past the whole typing burst.
pub fn record_undo(ctx: &egui::Context, id: Id, text: &str, selection: Range<usize>) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let mut undoer = state.undoer();
    undoer.add_undo(&(
        CCursorRange::two(CCursor::new(selection.start), CCursor::new(selection.end)),
        text.to_owned(),
    ));
    state.set_undoer(undoer);
    state.store(ctx, id);
}

/// Applies `format` to the editor's selection and keeps it selected.
pub fn apply_format(ctx: &egui::Context, id: Id, text: &mut String, format: Format) {
    let current = selection(ctx, id, text);
    record_undo(ctx, id, text, current.clone());
    let (edited, range) = compose_format::apply(text, current, format);
    *text = edited;
    set_selection(ctx, id, range);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// Inserts `insert` at the caret (over a selection) and puts the caret after.
pub fn insert_at_caret(ctx: &egui::Context, id: Id, text: &mut String, insert: &str) {
    let current = selection(ctx, id, text);
    record_undo(ctx, id, text, current.clone());
    let (edited, caret) = replace_range(text, current, insert);
    *text = edited;
    set_selection(ctx, id, caret..caret);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// The name shown on hover, with the shortcut when there is one.
fn tooltip(name: &str, format: Format) -> String {
    let command = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    let keys = match format {
        Format::Bold => "B",
        Format::Italic => "I",
        Format::Underline => "U",
        Format::Strike => "Shift+X",
        _ => return name.to_owned(),
    };
    format!("{name} ({command}+{keys})")
}

/// What the composer's placeholder ends with when there is room for it.
const MENTION_HINT: &str = "  ·  @ to mention";

/// `hint` for an editor `width` points wide: the short form, without
/// "· @ to mention", when the full one does not fit on one line.
pub fn fit_hint(ui: &Ui, hint: &str, width: f32) -> String {
    let measure = |text: &str| text_width(ui, text, theme::regular(14.0));
    shorten_hint(hint, width, measure)
}

fn shorten_hint(hint: &str, width: f32, measure: impl Fn(&str) -> f32) -> String {
    match hint.strip_suffix(MENTION_HINT) {
        Some(short) if measure(hint) > width => short.to_owned(),
        _ => hint.to_owned(),
    }
}

/// The markdown legend under the composer.
const LEGEND: &str =
    "**bold**  *italic*  `code`  > quote  - list  @ mention   ·   Shift+Enter for a new line";
const NEWLINE_HINT: &str = "Shift+Enter for a new line";

/// The line under the composer, always one line: the legend when it fits
/// and the toolbar is off, else just the new-line hint.
pub fn help_line(ui: &Ui, toolbar_on: bool) -> &'static str {
    let width = ui.available_width();
    choose_help(toolbar_on, |text| {
        text_width(ui, text, theme::regular(11.0)) <= width
    })
}

fn choose_help(toolbar_on: bool, fits: impl Fn(&str) -> bool) -> &'static str {
    if !toolbar_on && fits(LEGEND) {
        LEGEND
    } else {
        NEWLINE_HINT
    }
}

fn text_width(ui: &Ui, text: &str, font: egui::FontId) -> f32 {
    ui.ctx()
        .fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, egui::Color32::PLACEHOLDER))
        .size()
        .x
}

fn format_on_key() -> Id {
    Id::new("chaat-format-toolbar-on")
}

/// Whether the format toolbar is on. It is a session choice kept in egui's
/// memory, the same for every chat, off at start.
pub fn format_on(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(format_on_key()))
        .unwrap_or(false)
}

pub fn set_format_on(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(format_on_key(), on));
}

/// The "Formatting" button beside the emoji button: lit while the toolbar is
/// on.
pub fn format_toggle(ui: &mut Ui, palette: &Palette, on: bool) -> egui::Response {
    const SIZE: f32 = 30.0;
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(SIZE), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        let fill = if on {
            Some(palette.surface_active)
        } else if hovered {
            Some(palette.surface_hover)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(6), fill);
        }
        let tint = if on {
            palette.accent
        } else if hovered {
            palette.text
        } else {
            palette.secondary
        };
        Icon::Typography.image(tint, 18.0).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(18.0)),
        );
    }
    response
        .on_hover_text("Formatting")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The row of format buttons inside the composer, above the editor. Returns
/// the format clicked. Applying it restores the editor's focus and
/// selection (see [`apply_format`]).
pub fn format_row(ui: &mut Ui, palette: &Palette) -> Option<Format> {
    const SIZE: f32 = 24.0;
    let items = [
        (Icon::Bold, "Bold", Format::Bold),
        (Icon::Italic, "Italic", Format::Italic),
        (Icon::Underline, "Underline", Format::Underline),
        (Icon::Strikethrough, "Strikethrough", Format::Strike),
        (Icon::Code, "Code", Format::Code),
        (Icon::CodeBlock, "Code block", Format::CodeBlock),
        (Icon::Quote, "Quote", Format::Quote),
        (Icon::List, "Bulleted list", Format::Bullet),
        (Icon::ListOrdered, "Numbered list", Format::Numbered),
    ];
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.spacing_mut().interact_size.y = SIZE;
        for (icon, name, format) in items {
            let response = widgets::icon_button_sized(
                ui,
                palette,
                icon,
                &tooltip(name, format),
                SIZE,
                14.0,
                palette.secondary,
            );
            if response.clicked() {
                clicked = Some(format);
            }
        }
    });
    clicked
}

/// Turns a `:shortcode:` or `:) ` just typed before the editor's caret into
/// its emoji, as one undo step. Does nothing unless `allowed` (text was
/// typed this frame, no mention list is open).
pub fn convert_typed_at_caret(ctx: &egui::Context, id: Id, text: &mut String, allowed: bool) {
    if !allowed {
        return;
    }
    let Some(at) = egui::TextEdit::load_state(ctx, id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.primary.index.0)
    else {
        return;
    };
    let Some((converted, caret)) = convert_typed(text, at) else {
        return;
    };
    // Ctrl+Z gives back what was typed, in one step.
    record_undo(ctx, id, text, at..at);
    *text = converted;
    set_selection(ctx, id, caret..caret);
    ctx.request_repaint();
}

/// `text` with an emoticon or shortcode at its very end (or just before
/// trailing spaces) turned into its emoji, for when Enter sends. Code spans
/// and links are never touched.
pub fn convert_at_end(text: &str) -> Option<String> {
    let end = text.chars().count();
    let (range, emoji) = crate::emoji::convert_before_cursor(text, end)?;
    Some(replace_range(text, range, emoji).0)
}

/// The editor's galley with markdown styled in place. The styled job is
/// memoized under `id` until the text, width or colors change; egui's own
/// galley cache then makes laying it out free, and still notices when fonts
/// change.
pub fn styled_galley(
    ui: &Ui,
    palette: &Palette,
    id: Id,
    text: &str,
    wrap_width: f32,
) -> Arc<egui::Galley> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    wrap_width.to_bits().hash(&mut hasher);
    palette.text.hash(&mut hasher);
    palette.dim.hash(&mut hasher);
    let key = hasher.finish();
    let cached = ui.ctx().data(|d| d.get_temp::<(u64, Arc<LayoutJob>)>(id));
    let job = match cached {
        Some((cached_key, job)) if cached_key == key => job,
        _ => {
            let job = Arc::new(crate::compose_style::layout(
                text,
                theme::regular(14.0),
                palette.text,
                palette.dim,
                wrap_width,
            ));
            ui.ctx()
                .data_mut(|d| d.insert_temp(id, (key, Arc::clone(&job))));
            job
        }
    };
    ui.fonts_mut(|fonts| fonts.layout_job((*job).clone()))
}

/// Opens the emoji picker from `button` and returns the pick, if any. The
/// popup closes on a pick or a click outside it.
pub fn emoji_popup(
    palette: &Palette,
    button: &egui::Response,
    popup_id: Id,
    picker_id: Id,
) -> Option<String> {
    let mut picked = None;
    Popup::from_toggle_button_response(button)
        .id(popup_id)
        .kind(egui::PopupKind::Popup)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .gap(4.0)
        .show(|ui| {
            picked = emoji_picker::show(ui, palette, picker_id);
        });
    if picked.is_some() {
        Popup::close_id(&button.ctx, popup_id);
    }
    if !Popup::is_id_open(&button.ctx, popup_id) {
        emoji_picker::reset(&button.ctx, picker_id);
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn shortcut_is_consumed() {
        let mut events = vec![
            Event::Text("a".into()),
            key(Key::B, Modifiers::COMMAND),
            key(Key::Enter, Modifiers::NONE),
        ];
        assert_eq!(take_shortcut(&mut events), Some(Format::Bold));
        assert_eq!(events.len(), 2, "the Ctrl+B event is gone");
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, Event::Key { key: Key::B, .. }))
        );
        assert_eq!(take_shortcut(&mut events), None);
    }

    #[test]
    fn other_keys_are_left_alone() {
        let mut events = vec![
            key(Key::B, Modifiers::NONE),
            key(Key::K, Modifiers::COMMAND),
        ];
        assert_eq!(take_shortcut(&mut events), None);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn undo_after_an_outside_edit_restores_the_typed_text() {
        let ctx = egui::Context::default();
        let id = Id::new("undo-test");
        let mut text = "ok :)".to_owned();
        set_selection(&ctx, id, 5..5);
        record_undo(&ctx, id, &text, 5..5);
        text = "ok \u{1F642}".to_owned();
        let state = egui::TextEdit::load_state(&ctx, id).unwrap();
        let mut undoer = state.undoer();
        let back = undoer
            .undo(&(CCursorRange::one(CCursor::new(4)), text))
            .cloned();
        assert_eq!(back.map(|(_, t)| t), Some("ok :)".to_owned()));
    }

    #[test]
    fn replace_range_uses_chars() {
        assert_eq!(replace_range("é✓ x", 1..2, "🙂"), ("é🙂 x".to_owned(), 2));
        assert_eq!(replace_range("ab", 5..9, "c"), ("abc".to_owned(), 3));
    }

    #[test]
    fn shortcode_converts_at_the_caret() {
        assert_eq!(
            convert_typed("é :smile: rest", 9),
            Some(("é \u{1F604} rest".to_owned(), 3))
        );
    }

    #[test]
    fn emoticon_waits_for_the_space() {
        assert_eq!(convert_typed("ok :)", 5), None);
        assert_eq!(
            convert_typed("ok :) ", 6),
            Some(("ok \u{1F642} ".to_owned(), 5))
        );
    }

    #[test]
    fn nothing_to_convert() {
        assert_eq!(convert_typed("hello", 5), None);
        assert_eq!(convert_typed("`:smile:`", 8), None);
    }

    #[test]
    fn tooltips_name_the_shortcut() {
        let t = tooltip("Bold", Format::Bold);
        assert!(t.starts_with("Bold (") && t.ends_with("+B)"), "{t}");
        assert!(tooltip("Strikethrough", Format::Strike).ends_with("+Shift+X)"));
        assert_eq!(tooltip("Quote", Format::Quote), "Quote");
    }

    #[test]
    fn the_placeholder_drops_its_mention_hint_when_it_does_not_fit() {
        let by_chars = |text: &str| text.chars().count() as f32;
        let full = "Message Ada  ·  @ to mention";
        assert_eq!(shorten_hint(full, 100.0, by_chars), full);
        assert_eq!(shorten_hint(full, 20.0, by_chars), "Message Ada");
        // Other placeholders are left alone.
        assert_eq!(
            shorten_hint("Reply to the thread", 5.0, by_chars),
            "Reply to the thread"
        );
    }

    #[test]
    fn the_help_line_is_one_short_line_when_cramped_or_when_the_toolbar_is_on() {
        assert_eq!(choose_help(false, |_| true), LEGEND);
        assert_eq!(choose_help(false, |_| false), NEWLINE_HINT);
        assert_eq!(choose_help(true, |_| true), NEWLINE_HINT);
    }

    #[test]
    fn format_toolbar_is_off_until_turned_on() {
        let ctx = egui::Context::default();
        assert!(!format_on(&ctx));
        set_format_on(&ctx, true);
        assert!(format_on(&ctx));
        set_format_on(&ctx, false);
        assert!(!format_on(&ctx));
    }

    /// One headless frame with the row; returns what it reports.
    fn row_frame(ctx: &egui::Context, events: Vec<Event>) -> (Option<Format>, egui::Pos2) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 300.0),
            )),
            events,
            ..Default::default()
        };
        let palette = Palette::dark();
        let mut out = (None, egui::Pos2::ZERO);
        let mut full = ctx.run_ui(input, |ui| {
            out.1 = ui.cursor().min;
            out.0 = format_row(ui, &palette);
        });
        full.textures_delta.clear();
        out
    }

    #[test]
    fn format_row_reports_a_click() {
        let ctx = egui::Context::default();
        crate::ui::install(&ctx, true);
        let (_, origin) = row_frame(&ctx, Vec::new());
        let at = origin + egui::vec2(12.0, 12.0);
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        row_frame(&ctx, vec![Event::PointerMoved(at)]);
        row_frame(&ctx, vec![press(true)]);
        let (clicked, _) = row_frame(&ctx, vec![press(false)]);
        assert_eq!(clicked, Some(Format::Bold));
    }

    #[test]
    fn enter_converts_a_trailing_emoticon() {
        assert_eq!(convert_at_end("ok :)"), Some("ok \u{1F642}".to_owned()));
        assert_eq!(convert_at_end("ok :) "), Some("ok \u{1F642} ".to_owned()));
        assert_eq!(
            convert_at_end("hi :smile:"),
            Some("hi \u{1F604}".to_owned())
        );
        assert_eq!(convert_at_end("plain"), None);
        assert_eq!(convert_at_end("`ok :)"), None);
        assert_eq!(convert_at_end("see http://x.io:)"), None);
        assert_eq!(convert_at_end("a:)"), None);
    }
}
