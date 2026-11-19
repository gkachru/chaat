//! Draws [`RichText`] with egui: each block is one laid-out galley, so text
//! stays selectable, wraps natively and costs one shape per paragraph.
//! Links are found under the pointer from the galley's character positions.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use std::ops::Range;

use egui::{Align, Color32, CornerRadius, Margin, Sense, Stroke, Ui, vec2};

use crate::app::{App, Image};
use crate::html::{Block, RichText, Span};
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

/// Lays out spans into a job, returning the link of each character range.
fn job(
    spans: &[Span],
    look: &Look,
    palette: &Palette,
    width: f32,
) -> (LayoutJob, Vec<(usize, usize, String)>) {
    let mut job = LayoutJob::default();
    let mut links = Vec::new();
    let mut chars = 0;
    for span in spans {
        let style = &span.style;
        let font = if style.code {
            theme::mono(look.size - 1.0)
        } else if style.bold || style.mention {
            theme::semibold(look.size)
        } else {
            theme::regular(look.size)
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
            line_height: Some(look.size * 1.42),
            ..Default::default()
        };
        let count = span.text.chars().count();
        if let Some(link) = &style.link {
            links.push((chars, chars + count, link.clone()));
        }
        chars += count;
        let size = if look.jumbo {
            JUMBO_EMOJI
        } else {
            emoji_size(look.size)
        };
        let emoji_format = TextFormat {
            font_id: egui::FontId::new(size, format.font_id.family.clone()),
            valign: Align::Center,
            line_height: if look.jumbo {
                Some(size * 1.2)
            } else {
                format.line_height
            },
            ..format.clone()
        };
        for (range, emoji) in emoji_runs(&span.text) {
            let format = if emoji { &emoji_format } else { &format };
            job.append(&span.text[range], 0.0, format.clone());
        }
    }
    job.wrap = TextWrapping {
        max_width: width,
        ..Default::default()
    };
    (job, links)
}

/// One paragraph: selectable text with clickable links.
fn paragraph(
    ui: &mut Ui,
    spans: &[Span],
    look: &Look,
    palette: &Palette,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let (job, links) = job(spans, look, palette, width);
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let response = ui.add(
        egui::Label::new(galley.clone())
            .selectable(true)
            .sense(Sense::click()),
    );
    if links.is_empty() {
        return;
    }
    let Some(pointer) = response.hover_pos() else {
        return;
    };
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
    let palette = &app.palette;
    ui.spacing_mut().item_spacing.y = 4.0;
    for block in &text.blocks {
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
                egui::Frame::new()
                    .fill(palette.surface_hover)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(width - 20.0);
                        let mut job = LayoutJob::single_section(
                            code.clone(),
                            TextFormat {
                                font_id: theme::mono(12.0),
                                color: palette.text,
                                ..Default::default()
                            },
                        );
                        job.wrap.max_width = width - 20.0;
                        ui.add(egui::Label::new(job).selectable(true));
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

    #[test]
    fn emoji_are_one_step_up_the_type_scale() {
        assert_eq!(emoji_size(12.0), 13.0);
        assert_eq!(emoji_size(13.0), 14.0);
        assert_eq!(emoji_size(14.0), 15.0);
        assert_eq!(emoji_size(15.0), 18.0);
        assert_eq!(emoji_size(30.0), 30.0);
    }
}
