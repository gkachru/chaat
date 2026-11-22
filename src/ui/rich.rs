//! Draws [`RichText`] with egui: each block is one laid-out galley, so text
//! stays selectable, wraps natively and costs one shape per paragraph.
//! Links are found under the pointer from the galley's character positions.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use std::ops::Range;

use egui::{Align, Color32, CornerRadius, Margin, Sense, Stroke, Ui, vec2};

use crate::app::{App, Image};
use crate::html::{Block, RichText, Script, Span};
use crate::model::Action;
use crate::theme::{self, Palette};

pub struct Look {
    pub size: f32,
    pub color: Color32,
    /// A message of nothing but a few emoji: they draw at 30 points.
    pub jumbo: bool,
}

impl Look {
    pub fn body(palette: &Palette) -> Self {
        Self {
            size: 14.0,
            color: palette.text,
            jumbo: false,
        }
    }
}

/// The sizes the interface uses (see AGENTS.md).
const TYPE_SCALE: [f32; 9] = [11.0, 12.0, 13.0, 14.0, 15.0, 18.0, 20.0, 24.0, 30.0];

/// One step down the type scale, for raised and lowered text.
fn step_down(size: f32) -> f32 {
    TYPE_SCALE
        .into_iter()
        .rev()
        .find(|s| *s < size)
        .unwrap_or(size)
}

/// Emoji draw one step up the type scale from the text around them, as
/// Teams does, so they read at the same weight as letters.
fn emoji_size(size: f32) -> f32 {
    TYPE_SCALE.into_iter().find(|s| *s > size).unwrap_or(size)
}

/// Size of emoji in a message that holds nothing else.
const JUMBO_EMOJI: f32 = 30.0;

/// Most emoji a message can hold and still draw large.
const JUMBO_LIMIT: usize = 10;

/// Whether the text is one to ten emoji and nothing but whitespace besides.
pub fn is_emoji_only(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let (mut i, mut count) = (0, 0);
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
        } else if let Some(n) = emoji_at(&chars, i) {
            i += n;
            count += 1;
        } else {
            return false;
        }
    }
    (1..=JUMBO_LIMIT).contains(&count)
}

/// Whether a body is only emoji: plain paragraphs without links, mentions,
/// code or images.
pub fn body_is_emoji_only(body: &RichText) -> bool {
    let mut text = String::new();
    for block in &body.blocks {
        match block {
            Block::Paragraph(spans)
                if spans
                    .iter()
                    .all(|s| s.style.link.is_none() && !s.style.mention && !s.style.code) =>
            {
                for span in spans {
                    text.push_str(&span.text);
                }
                text.push('\n');
            }
            _ => return false,
        }
    }
    is_emoji_only(&text)
}

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Characters shown as emoji without a variation selector.
fn is_emoji_default(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1F1E5 | 0x1F200..=0x1FAFF
        | 0x231A..=0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3
        | 0x25FD..=0x25FE | 0x2614..=0x2615 | 0x2648..=0x2653 | 0x267F
        | 0x2693 | 0x26A1 | 0x26AA..=0x26AB | 0x26BD..=0x26BE
        | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3
        | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A..=0x270B | 0x2728
        | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797
        | 0x27B0 | 0x27BF | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55)
}

/// Characters that are emoji only when a variation selector (FE0F) follows.
fn is_emoji_text_default(c: char) -> bool {
    matches!(c as u32,
        0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x21AA
        | 0x2328 | 0x23CF | 0x23ED..=0x23EF | 0x23F1..=0x23F2 | 0x23F8..=0x23FA
        | 0x24C2 | 0x25AA..=0x25AB | 0x25B6 | 0x25C0 | 0x25FB..=0x25FC
        | 0x2600..=0x27BF | 0x2934..=0x2935 | 0x2B05..=0x2B07
        | 0x3030 | 0x303D | 0x3297 | 0x3299)
}

/// Length in characters of the emoji starting at `chars[i]`, or None when
/// none starts there: pictographs with their selector and skin tone,
/// keycaps, flags and ZWJ sequences.
fn emoji_at(chars: &[char], i: usize) -> Option<usize> {
    const VS16: char = '\u{FE0F}';
    const ZWJ: char = '\u{200D}';
    const KEYCAP: char = '\u{20E3}';
    let at = |n: usize| chars.get(n).copied();
    let first = chars[i];
    if first.is_ascii_digit() || first == '#' || first == '*' {
        // The key, an optional FE0F, then the combining keycap.
        let mut n = i + 1;
        if at(n) == Some(VS16) {
            n += 1;
        }
        return (at(n) == Some(KEYCAP)).then_some(n + 1 - i);
    }
    if is_regional_indicator(first) {
        return Some(if at(i + 1).is_some_and(is_regional_indicator) {
            2
        } else {
            1
        });
    }
    let starts = |n: usize| {
        at(n).is_some_and(|c| {
            is_emoji_default(c) || (is_emoji_text_default(c) && at(n + 1) == Some(VS16))
        })
    };
    let mut end = i;
    // One emoji: a pictograph with its marks, then more after each joiner.
    while starts(end) {
        end += 1;
        // Selector, skin tone, keycap mark, tags.
        while at(end).is_some_and(
            |m| matches!(m, VS16 | KEYCAP | '\u{1F3FB}'..='\u{1F3FF}' | '\u{E0020}'..='\u{E007F}'),
        ) {
            end += 1;
        }
        // A joiner continues the sequence only into another emoji.
        if at(end) == Some(ZWJ) && starts(end + 1) {
            end += 1;
        } else {
            break;
        }
    }
    (end > i).then_some(end - i)
}

/// Splits text into runs, each flagged whether it is emoji. The ranges are
/// byte offsets that cover the whole string.
pub fn emoji_runs(text: &str) -> Vec<(Range<usize>, bool)> {
    let chars: Vec<char> = text.chars().collect();
    let mut offsets: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    offsets.push(text.len());
    let mut runs: Vec<(Range<usize>, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (len, emoji) = match emoji_at(&chars, i) {
            Some(n) => (n, true),
            None => (1, false),
        };
        let range = offsets[i]..offsets[i + len];
        match runs.last_mut() {
            Some((last, flag)) if *flag == emoji => last.end = range.end,
            _ => runs.push((range, emoji)),
        }
        i += len;
    }
    runs
}

/// How far superscript rises and subscript drops, in ems of the text around.
const RAISE: f32 = 0.25;
const DROP: f32 = 0.2;
/// A font's ascent in ems (close enough across the interface fonts).
const ASCENT: f32 = 0.95;

/// The line height that puts a script section's baseline off the text's.
///
/// The text layout puts a glyph's baseline at its ascent plus the row height
/// minus its own line height (bottom aligned, the default), whatever else is
/// in the row. So a section with a smaller line height sits lower and one
/// with a larger line height higher, in a row of any height. Plain text has
/// `base * 1.42`; a script of font size `size` needs that, corrected for its
/// smaller ascent, plus the rise (or minus the drop). Raising grows the row
/// by a pixel or two: nothing sits above a row's top.
fn script_line_height(base: f32, size: f32, script: Script) -> f32 {
    let shift = match script {
        Script::Sup => RAISE * base,
        Script::Sub => -DROP * base,
    };
    (base * 1.42 + ASCENT * (size - base) + shift).max(size)
}

/// Appends `text`, with the occurrences of the find query marked.
fn append_marked(
    job: &mut LayoutJob,
    text: &str,
    format: &TextFormat,
    query: &str,
    palette: &Palette,
) {
    let mut at = 0;
    for range in super::find::matches(text, query) {
        if range.start > at {
            job.append(&text[at..range.start], 0.0, format.clone());
        }
        let marked = TextFormat {
            background: super::find::highlight_color(palette),
            ..format.clone()
        };
        job.append(&text[range.clone()], 0.0, marked);
        at = range.end;
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, format.clone());
    }
}

/// Lays out spans into a job, returning the link of each character range.
fn job(
    spans: &[Span],
    look: &Look,
    palette: &Palette,
    width: f32,
    find: Option<&str>,
) -> (LayoutJob, Vec<(usize, usize, String)>) {
    let mut job = LayoutJob::default();
    let mut links = Vec::new();
    let mut chars = 0;
    for span in spans {
        let style = &span.style;
        let base = style.size.map_or(look.size, f32::from);
        let size = if style.script.is_some() {
            step_down(base)
        } else {
            base
        };
        let font = if style.code {
            theme::mono(size - 1.0)
        } else if style.bold || style.mention {
            theme::semibold(size)
        } else {
            theme::regular(size)
        };
        let color = if style.link.is_some() {
            palette.link
        } else if style.mention {
            palette.accent
        } else {
            look.color
        };
        let format = TextFormat {
            font_id: font,
            color,
            italics: style.italic,
            underline: if style.underline || style.link.is_some() {
                Stroke::new(1.0, color.gamma_multiply(0.6))
            } else {
                Stroke::NONE
            },
            strikethrough: if style.strike {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            },
            background: if style.code {
                palette.surface_active
            } else if style.mention {
                palette.highlight
            } else {
                Color32::TRANSPARENT
            },
            line_height: Some(match style.script {
                Some(script) => script_line_height(base, size, script),
                None => base * 1.42,
            }),
            ..Default::default()
        };
        let count = span.text.chars().count();
        if let Some(link) = &style.link {
            links.push((chars, chars + count, link.clone()));
        }
        chars += count;
        let emoji_px = if look.jumbo {
            JUMBO_EMOJI
        } else {
            emoji_size(base)
        };
        let emoji_format = TextFormat {
            font_id: egui::FontId::new(emoji_px, format.font_id.family.clone()),
            valign: Align::Center,
            line_height: if look.jumbo {
                Some(emoji_px * 1.2)
            } else {
                format.line_height
            },
            ..format.clone()
        };
        for (range, emoji) in emoji_runs(&span.text) {
            let format = if emoji { &emoji_format } else { &format };
            let text = &span.text[range];
            match find.filter(|_| !emoji) {
                Some(query) => append_marked(&mut job, text, format, query, palette),
                None => job.append(text, 0.0, format.clone()),
            }
        }
    }
    job.wrap = TextWrapping {
        max_width: width,
        ..Default::default()
    };
    (job, links)
}

thread_local! {
    /// While [`show`] draws: the widest paragraph seen in the last frame
    /// (what paragraphs stretch to) and in this one.
    static WIDEST: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

/// One paragraph: selectable text with clickable links. It stretches to the
/// widest paragraph of the message, so a drag that ends beside a short line
/// still selects it, while the message keeps the width its text gave it.
fn paragraph(
    ui: &mut Ui,
    spans: &[Span],
    look: &Look,
    palette: &Palette,
    width: f32,
    actions: &mut Vec<Action>,
) {
    paragraph_in(ui, spans, look, palette, width, true, actions);
}

fn paragraph_in(
    ui: &mut Ui,
    spans: &[Span],
    look: &Look,
    palette: &Palette,
    width: f32,
    fill: bool,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let find = super::find::active_query(ui.ctx());
    let (job, links) = job(spans, look, palette, width, find.as_deref());
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let label = egui::Label::new(galley.clone())
        .selectable(true)
        .sense(Sense::click());
    let response = if fill {
        let (previous, widest) = WIDEST.get();
        WIDEST.set((previous, widest.max(galley.size().x)));
        let stretch = previous.min(width).max(galley.size().x);
        ui.allocate_ui_with_layout(
            vec2(stretch, 0.0),
            egui::Layout::top_down_justified(Align::Min),
            |ui| ui.add(label),
        )
        .inner
    } else {
        ui.add(label)
    };
    if links.is_empty() {
        return response;
    }
    let Some(pointer) = response.hover_pos() else {
        return response;
    };
    // Beside the text there is no link.
    if !galley
        .rect
        .contains((pointer - response.rect.min).to_pos2())
    {
        return response;
    }
    let cursor = galley.cursor_from_pos(pointer - response.rect.min);
    let index = cursor.index.0;
    // The character under the pointer, not the gap after it.
    let glyph = galley.pos_from_cursor(cursor);
    let index = if pointer.x < response.rect.min.x + glyph.min.x {
        index.saturating_sub(1)
    } else {
        index
    };
    if let Some((_, _, link)) = links
        .iter()
        .find(|(start, end, _)| (*start..*end).contains(&index))
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        response.clone().on_hover_text_at_pointer(link.as_str());
        if response.clicked() {
            actions.push(Action::OpenUrl(link.clone()));
        }
    }
    response
}

/// What dragging over the whole message and copying gives: the text of each
/// block on its own line. Bullets, numbers and quote marks are drawn apart
/// from the text, so they are left out as they are in a drag.
pub fn selectable_text(text: &RichText) -> String {
    let join = |spans: &[Span]| spans.iter().map(|s| s.text.as_str()).collect::<String>();
    let lines: Vec<String> = text
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph(spans)
            | Block::Heading(spans)
            | Block::Quote(spans)
            | Block::Item { spans, .. } => Some(join(spans)),
            Block::Code(code) | Block::Snippet { text: code, .. } => Some(code.clone()),
            Block::Table { rows, .. } => Some(
                rows.iter()
                    .map(|row| row.iter().map(|c| join(c)).collect::<Vec<_>>().join(" | "))
                    .collect::<Vec<_>>()
                    .join(
                        "
",
                    ),
            ),
            Block::Image { .. } | Block::Rule => None,
        })
        .collect();
    lines.join(
        "
",
    )
}

fn content_hash(text: &RichText) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    selectable_text(text).hash(&mut hasher);
    hasher.finish()
}

/// Draws a message body within `width`.
pub fn show(
    ui: &mut Ui,
    app: &App,
    text: &RichText,
    look: &Look,
    width: f32,
    actions: &mut Vec<Action>,
) {
    ui.spacing_mut().item_spacing.y = 4.0;
    // Keyed by content and width, not by position, so a message that moves
    // in the list never reads another message's width.
    let key = egui::Id::new((
        "rich-widest",
        content_hash(text),
        width.to_bits(),
        look.size.to_bits(),
    ));
    let known: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(0.0);
    let outer = WIDEST.replace((known, 0.0));
    show_blocks(ui, app, text, look, width, actions);
    let (_, widest) = WIDEST.replace(outer);
    if (widest - known).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(key, widest));
        ui.ctx().request_repaint();
    }
}

fn show_blocks(
    ui: &mut Ui,
    app: &App,
    text: &RichText,
    look: &Look,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    for (index, block) in text.blocks.iter().enumerate() {
        match block {
            Block::Paragraph(spans) => paragraph(ui, spans, look, palette, width, actions),
            Block::Heading(spans) => {
                let heading = Look {
                    size: look.size + 1.0,
                    color: look.color,
                    jumbo: false,
                };
                let bold: Vec<Span> = spans
                    .iter()
                    .map(|s| {
                        let mut s = s.clone();
                        s.style.bold = true;
                        s
                    })
                    .collect();
                paragraph(ui, &bold, &heading, palette, width, actions);
            }
            Block::Quote(spans) => {
                egui::Frame::new()
                    .inner_margin(Margin {
                        left: 10,
                        right: 4,
                        top: 2,
                        bottom: 2,
                    })
                    .show(ui, |ui| {
                        let rect = ui.max_rect();
                        let quote = Look {
                            size: look.size,
                            color: palette.secondary,
                            jumbo: false,
                        };
                        paragraph(ui, spans, &quote, palette, width - 14.0, actions);
                        let bar = egui::Rect::from_min_max(
                            rect.left_top() - vec2(10.0, 0.0),
                            egui::pos2(rect.left() - 7.0, ui.min_rect().bottom()),
                        );
                        ui.painter()
                            .rect_filled(bar, CornerRadius::same(2), palette.outline);
                    });
            }
            Block::Item { ordered, spans } => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let marker = match ordered {
                        Some(n) => format!("{n}."),
                        None => "•".into(),
                    };
                    ui.add_sized(
                        vec2(14.0, look.size * 1.42),
                        egui::Label::new(
                            egui::RichText::new(marker)
                                .font(theme::regular(look.size))
                                .color(palette.secondary),
                        ),
                    );
                    paragraph(ui, spans, look, palette, width - 20.0, actions);
                });
            }
            Block::Code(code) => {
                ui.push_id(("code", index), |ui| {
                    code_block(ui, palette, code, None, width, actions)
                });
            }
            Block::Snippet { language, text } => {
                ui.push_id(("code", index), |ui| {
                    code_block(ui, palette, text, language.as_deref(), width, actions)
                });
            }
            Block::Table { header, rows } => {
                ui.push_id(("table", index), |ui| {
                    table(ui, *header, rows, look, palette, width, actions)
                });
            }
            Block::Image {
                src,
                alt,
                width: w,
                height: h,
            } => image(ui, app, src, alt.as_deref(), *w, *h, width, actions),
            Block::Rule => {
                ui.add_space(2.0);
                let y = ui.cursor().top();
                ui.painter().hline(
                    ui.cursor().left()..=ui.cursor().left() + width,
                    y,
                    Stroke::new(1.0, palette.outline),
                );
                ui.add_space(4.0);
            }
        }
    }
}

fn code_block(
    ui: &mut Ui,
    palette: &Palette,
    code: &str,
    language: Option<&str>,
    width: f32,
    actions: &mut Vec<Action>,
) {
    ui.scope(|ui| {
        ui.set_max_width(width);
        super::code::block(ui, palette, code, language, actions);
    });
}

/// Vertical gap between table rows; the divider lines sit in it.
const ROW_GAP: f32 = 8.0;

/// Widest a table cell grows before its text wraps.
const CELL_WIDTH: f32 = 220.0;

/// A table in a grid. When its columns are wider than the bubble it
/// scrolls sideways inside it.
fn table(
    ui: &mut Ui,
    header: bool,
    rows: &[Vec<Vec<Span>>],
    look: &Look,
    palette: &Palette,
    width: f32,
    actions: &mut Vec<Action>,
) {
    // A solid scroll bar, there only when the table is wider than the room.
    ui.scope(|ui| {
        super::code::thin_scrollbars(ui, palette);
        egui::ScrollArea::horizontal()
            .id_salt("scroll")
            .max_width(width)
            .auto_shrink([true, true])
            .show(ui, |ui| {
                egui::Frame::new()
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        // Top of each row, from its cells.
                        let mut tops: Vec<f32> = Vec::new();
                        let grid =
                            egui::Grid::new("grid")
                                .spacing(vec2(16.0, ROW_GAP))
                                .show(ui, |ui| {
                                    for (row_index, row) in rows.iter().enumerate() {
                                        let mut top = f32::MAX;
                                        for cell in row {
                                            if header && row_index == 0 {
                                                let bold: Vec<Span> = cell
                                                    .iter()
                                                    .map(|s| {
                                                        let mut s = s.clone();
                                                        s.style.bold = true;
                                                        s
                                                    })
                                                    .collect();
                                                let at = paragraph_in(
                                                    ui, &bold, look, palette, CELL_WIDTH, false,
                                                    actions,
                                                );
                                                top = top.min(at.rect.top());
                                            } else {
                                                let at = paragraph_in(
                                                    ui, cell, look, palette, CELL_WIDTH, false,
                                                    actions,
                                                );
                                                top = top.min(at.rect.top());
                                            }
                                        }
                                        if top.is_finite() && top < f32::MAX {
                                            tops.push(top);
                                        }
                                        ui.end_row();
                                    }
                                });
                        // A line between every two rows, in the gap between them.
                        for top in tops.iter().skip(1) {
                            let y = top - ROW_GAP / 2.0;
                            ui.painter().hline(
                                grid.response.rect.x_range(),
                                y,
                                Stroke::new(1.0, palette.outline),
                            );
                        }
                    });
            });
    });
}

#[allow(clippy::too_many_arguments)]
fn image(
    ui: &mut Ui,
    app: &App,
    src: &str,
    alt: Option<&str>,
    w: Option<f32>,
    h: Option<f32>,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    let max = vec2(width.min(360.0), 280.0);
    let natural = match (w, h) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => vec2(w, h),
        _ => vec2(240.0, 160.0),
    };
    let scale = (max.x / natural.x).min(max.y / natural.y).min(1.0);
    let size = natural * scale;
    match app.media.get(src) {
        Some(Image::Ready(uri)) => {
            let response = ui
                .add(
                    egui::Image::new(uri.clone())
                        .fit_to_exact_size(size)
                        .corner_radius(CornerRadius::same(6))
                        .sense(Sense::click())
                        .alt_text(alt.unwrap_or("Image")),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Open image preview");
            super::widgets::hover_wash(ui, palette, &response, 6);
            if response.clicked() {
                actions.push(Action::PreviewImage(uri.clone()));
            }
            if ui.is_rect_visible(response.rect)
                && crate::images::needs_source(
                    ui.ctx(),
                    uri,
                    egui::load::SizeHint::Size {
                        width: (size.x * ui.ctx().pixels_per_point()).ceil() as u32,
                        height: (size.y * ui.ctx().pixels_per_point()).ceil() as u32,
                        maintain_aspect_ratio: true,
                    },
                )
            {
                actions.push(Action::LoadMedia(src.to_owned()));
            }
        }
        state => {
            let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
            let label = match state {
                Some(Image::Missing) => "Image unavailable",
                _ => "Loading image…",
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                theme::regular(12.0),
                palette.dim,
            );
            // Only images on screen are fetched.
            if state.is_none() && ui.is_rect_visible(response.rect) {
                actions.push(Action::LoadMedia(src.to_owned()));
            }
        }
    }
}

/// A one-line rich preview for quotes: plain text, secondary colour.
pub fn quote_block(
    ui: &mut Ui,
    palette: &Palette,
    author: Option<&str>,
    preview: &str,
    width: f32,
) -> egui::Response {
    egui::Frame::new()
        .fill(palette.surface_hover)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin {
            left: 10,
            right: 10,
            top: 5,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_max_width(width - 20.0);
            let rect = ui.max_rect();
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                if let Some(author) = author {
                    ui.label(
                        egui::RichText::new(author)
                            .font(theme::semibold(12.0))
                            .color(palette.secondary),
                    );
                }
                let mut job = LayoutJob::single_section(
                    preview.to_owned(),
                    TextFormat {
                        font_id: theme::regular(13.0),
                        color: palette.secondary,
                        ..Default::default()
                    },
                );
                job.wrap = TextWrapping {
                    max_width: width - 24.0,
                    max_rows: 3,
                    break_anywhere: false,
                    overflow_character: Some('…'),
                };
                ui.add(egui::Label::new(job));
            });
            let bar = egui::Rect::from_min_max(
                rect.left_top() - vec2(10.0, 5.0),
                egui::pos2(rect.left() - 7.0, ui.min_rect().bottom() + 6.0),
            );
            ui.painter().rect_filled(
                bar,
                CornerRadius {
                    nw: 6,
                    sw: 6,
                    ne: 0,
                    se: 0,
                },
                palette.accent,
            );
        })
        .response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runs of `text`, as (text, is_emoji).
    fn runs(text: &str) -> Vec<(&str, bool)> {
        emoji_runs(text)
            .into_iter()
            .map(|(range, emoji)| (&text[range], emoji))
            .collect()
    }

    #[test]
    fn plain_text_is_one_run() {
        assert_eq!(runs("Ship it"), [("Ship it", false)]);
        assert!(runs("").is_empty());
    }

    #[test]
    fn emoji_between_words_split_the_text() {
        assert_eq!(runs("🚀"), [("🚀", true)]);
        assert_eq!(
            runs("Ship it 🚀 now"),
            [("Ship it ", false), ("🚀", true), (" now", false)]
        );
        assert_eq!(runs("😀😀"), [("😀😀", true)]);
    }

    #[test]
    fn joined_tinted_and_flag_sequences_stay_whole() {
        let family = "👨\u{200D}👩\u{200D}👧";
        assert_eq!(runs(family), [(family, true)]);
        let wave = "👋\u{1F3FD}";
        assert_eq!(
            runs(&format!("a{wave}b")),
            [("a", false), (wave, true), ("b", false)]
        );
        assert_eq!(runs("🇮🇳🇺🇸"), [("🇮🇳🇺🇸", true)]);
        let heart = "❤\u{FE0F}";
        assert_eq!(runs(heart), [(heart, true)]);
        assert_eq!(runs("a\u{200D}b"), [("a\u{200D}b", false)]);
    }

    #[test]
    fn keycaps_count_but_bare_digits_and_symbols_do_not() {
        let one = "1\u{FE0F}\u{20E3}";
        assert_eq!(runs(&format!("x {one}")), [("x ", false), (one, true)]);
        assert_eq!(runs("#1 * 2"), [("#1 * 2", false)]);
        assert_eq!(runs("© 2025 ™ ® ❤ ✔"), [("© 2025 ™ ® ❤ ✔", false)]);
        let tm = "™\u{FE0F}";
        assert_eq!(runs(&format!("a{tm}")), [("a", false), (tm, true)]);
    }

    #[test]
    fn messages_of_only_a_few_emoji_are_jumbo() {
        assert!(is_emoji_only("🚀"));
        assert!(is_emoji_only("👍👍 🎉"));
        assert!(is_emoji_only("❤\u{FE0F}"));
        assert!(is_emoji_only(" 🇮🇳 "));
        assert!(!is_emoji_only("🚀 ok"));
        assert!(!is_emoji_only("1"));
        assert!(!is_emoji_only(""));
        assert!(!is_emoji_only("  "));
        assert!(is_emoji_only(&"🎉".repeat(10)));
        assert!(!is_emoji_only(&"🎉".repeat(11)));
        assert!(!body_is_emoji_only(&RichText::plain("ok 🚀")));
        assert!(body_is_emoji_only(&RichText::plain("🚀\n\n🎉")));
        let mut link = RichText::plain("🚀");
        if let Block::Paragraph(spans) = &mut link.blocks[0] {
            spans[0].style.link = Some("https://example.com".into());
        }
        assert!(!body_is_emoji_only(&link));
    }

    fn spans_of(html: &str) -> Vec<Span> {
        match crate::html::parse(html).blocks.remove(0) {
            Block::Paragraph(spans) => spans,
            other => panic!("paragraph expected, got {other:?}"),
        }
    }

    #[test]
    fn sup_raised() {
        let palette = Palette::dark();
        let look = Look::body(&palette);
        let (job, _) = job(
            &spans_of("<p>x<sup>2</sup>y<sub>i</sub></p>"),
            &look,
            &palette,
            300.0,
            None,
        );
        let formats: Vec<_> = job.sections.iter().map(|s| &s.format).collect();
        let plain = formats[0].line_height.unwrap();
        let (sup, sub) = (
            formats[1].line_height.unwrap(),
            formats[3].line_height.unwrap(),
        );
        assert_eq!(formats[0].font_id.size, 14.0);
        assert_eq!(formats[1].font_id.size, 13.0);
        assert_eq!(formats[3].font_id.size, 13.0);
        // A larger line height sits higher in a bottom-aligned row.
        assert!(
            sup > plain - 2.0 && sup - plain < 4.0,
            "raise grows the row a little"
        );
        assert!(sub < plain, "lowered");
        assert!(
            formats
                .iter()
                .all(|f| f.valign == TextFormat::default().valign)
        );
        // Plain text around is untouched.
        assert_eq!(formats[2].line_height, formats[0].line_height);
    }

    #[test]
    fn find_marks_the_matched_parts_of_a_run() {
        let palette = Palette::dark();
        let look = Look::body(&palette);
        let spans = spans_of("<p>The theme: <b>other</b> ünï THE</p>");
        let (marked, _) = job(&spans, &look, &palette, 300.0, Some("the"));
        let (plain, _) = job(&spans, &look, &palette, 300.0, None);
        assert_eq!(marked.text, plain.text);
        let color = crate::ui::find::highlight_color(&palette);
        let pieces: Vec<(&str, bool)> = marked
            .sections
            .iter()
            .map(|s| {
                (
                    &marked.text[s.byte_range.start.0..s.byte_range.end.0],
                    s.format.background == color,
                )
            })
            .collect();
        assert_eq!(
            pieces,
            [
                ("The", true),
                (" ", false),
                ("the", true),
                ("me: ", false),
                ("o", false),
                ("the", true),
                ("r", false),
                (" ünï ", false),
                ("THE", true),
            ]
        );
        assert!(plain.sections.iter().all(|s| s.format.background != color));
        // No query, no marks; a query that is not there changes nothing.
        let (none, _) = job(&spans, &look, &palette, 300.0, Some("zzz"));
        assert_eq!(none.sections.len(), plain.sections.len());
    }

    #[test]
    fn script_height_never_below_its_font() {
        assert!(script_line_height(11.0, 11.0, Script::Sub) >= 11.0);
        assert!(script_line_height(30.0, 24.0, Script::Sup) > 30.0 * 1.42 - 24.0);
    }

    #[test]
    fn sized_spans_use_their_size_on_the_scale() {
        let palette = Palette::dark();
        let look = Look::body(&palette);
        let (job, _) = job(
            &spans_of("<p>a<span style=\"font-size:24px\">B<sup>c</sup></span></p>"),
            &look,
            &palette,
            300.0,
            None,
        );
        let sizes: Vec<f32> = job.sections.iter().map(|s| s.format.font_id.size).collect();
        assert_eq!(sizes[0], 14.0);
        assert_eq!(sizes[1], 24.0);
        assert_eq!(sizes[2], 20.0);
        assert_eq!(step_down(11.0), 11.0);
        assert_eq!(step_down(30.0), 24.0);
    }

    /// Draws `html` in a 300-point column; returns the width it used.
    fn frame(html: &str) -> f32 {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            crate::paths::AppDirs::under(dir.path()),
            crate::settings::Settings::default(),
            crate::backend::Waker::default(),
            true,
        );
        let ctx = egui::Context::default();
        crate::ui::install(&ctx, true);
        let body = crate::html::parse(html);
        let look = Look::body(&app.palette);
        let mut used = 0.0;
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(input, |ui| {
                ui.scope(|ui| {
                    ui.set_max_width(300.0);
                    show(ui, &app, &body, &look, 300.0, &mut actions);
                    used = ui.min_rect().width();
                });
            });
            output.textures_delta.clear();
            assert!(!output.shapes.is_empty());
        }
        used
    }

    #[test]
    fn every_new_block_draws() {
        frame("<codeblock class=\"language-rust\"><code>fn a() {}</code></codeblock>");
        frame("<pre>old style</pre>");
        frame("<p>x<sup>2</sup> H<sub>2</sub>O <span style=\"font-size:30px\">big</span></p>");
        frame("<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td></td></tr></table>");
        let mut old = RichText::default();
        old.blocks.push(Block::Code("legacy".into()));
        assert_eq!(old.to_plain(), "legacy");
    }

    #[test]
    fn plain_join() {
        let body = crate::html::parse(
            "<p>one <b>two</b></p><h2>Head</h2><ul><li>dot</li></ul><pre>a
b</pre><hr><table><tr><td>x</td><td>y</td></tr></table>",
        );
        assert_eq!(
            selectable_text(&body),
            "one two
Head
dot
a
b
x | y"
        );
        assert_eq!(selectable_text(&RichText::default()), "");
    }

    #[test]
    fn widest_settles_without_repaint_loop() {
        let (app, ctx, _dir) = app_and_ctx();
        let a = crate::html::parse("<p>a much longer first paragraph</p><p>b</p>");
        let b = crate::html::parse("<p>hi</p>");
        let look = Look::body(&app.palette);
        let mut repaints = Vec::new();
        for _ in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(input, |ui| {
                // Order swaps between frames: keys must not follow position.
                show(ui, &app, &a, &look, 300.0, &mut actions);
                show(ui, &app, &b, &look, 300.0, &mut actions);
            });
            output.textures_delta.clear();
            repaints.push(
                output
                    .viewport_output
                    .values()
                    .any(|v| v.repaint_delay.is_zero()),
            );
        }
        assert!(repaints[0], "first frame learns the width");
        assert!(
            !repaints[2] && !repaints[3],
            "still repainting: {repaints:?}"
        );
    }

    #[test]
    fn short_messages_keep_their_width() {
        let used = frame("<p>short</p><p>a</p>");
        assert!(used < 80.0, "paragraphs stretched the message to {used}");
    }

    #[test]
    fn wide_table_scrolls() {
        let cell = "<td>some wide cell text here</td>".repeat(12);
        let used = frame(&format!("<table><tr>{cell}</tr></table>"));
        assert!(used <= 300.5, "table pushed the bubble to {used}");
    }

    fn app_and_ctx() -> (App, egui::Context, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            crate::paths::AppDirs::under(dir.path()),
            crate::settings::Settings::default(),
            crate::backend::Waker::default(),
            true,
        );
        let ctx = egui::Context::default();
        crate::ui::install(&ctx, true);
        (app, ctx, dir)
    }

    #[test]
    fn drag_spans_paragraphs() {
        let (app, ctx, _dir) = app_and_ctx();
        let body = crate::html::parse("<p>alpha one is the longest</p><p>beta</p><p>gamma</p>");
        let look = Look::body(&app.palette);
        let run = |events: Vec<egui::Event>| {
            let mut rects: Vec<egui::Rect> = Vec::new();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(input, |ui| {
                let before = ui.cursor().top();
                show(ui, &app, &body, &look, 300.0, &mut actions);
                // Three paragraphs of equal height.
                let total = ui.min_rect().bottom() - before;
                rects = (0..3)
                    .map(|i| {
                        let h = total / 3.0;
                        egui::Rect::from_min_size(
                            egui::pos2(ui.min_rect().left(), before + h * i as f32),
                            vec2(300.0, h),
                        )
                    })
                    .collect();
            });
            output.textures_delta.clear();
            (output, rects)
        };
        let rects = run(vec![]).1;
        let start = rects[0].left_center() + vec2(1.0, 0.0);
        // Beside the short last line, inside the width of the longest.
        let end = rects[2].left_center() + vec2(100.0, 0.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(vec![egui::Event::PointerMoved(start)]);
        run(vec![button(start, true)]);
        run(vec![egui::Event::PointerMoved(rects[1].center())]);
        run(vec![egui::Event::PointerMoved(end)]);
        run(vec![]);
        run(vec![button(end, false)]);
        let (output, _) = run(vec![egui::Event::Copy]);
        let copied: String = output
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            copied,
            "alpha one is the longest
beta
gamma"
        );
    }

    #[test]
    fn emoji_are_one_step_up_the_type_scale() {
        assert_eq!(emoji_size(12.0), 13.0);
        assert_eq!(emoji_size(13.0), 14.0);
        assert_eq!(emoji_size(14.0), 15.0);
        assert_eq!(emoji_size(15.0), 18.0);
        assert_eq!(emoji_size(30.0), 30.0);
    }
}
