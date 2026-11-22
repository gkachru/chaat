//! Live styling of markdown in the composer.
//!
//! The text is never changed: markers stay in it, dimmed, so egui's own undo
//! and cursor handling keep working. The rules follow `html::compose`
//! (`**`, `*`, `_`, `~~`, backticks, fences, `- `, `1. `, `> `, bare links)
//! plus `++underline++`: what looks styled here is what gets sent styled.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Stroke};

use crate::theme;

const DIM: u8 = 1;
const BOLD: u8 = 2;
const ITALIC: u8 = 4;
const STRIKE: u8 = 8;
const UNDER: u8 = 16;
const CODE: u8 = 32;
const LINK: u8 = 64;

/// A layout for `text` with markdown styled in place. `font` is the body
/// font, `dim` colors the markers. Sections are byte ranges that cover the
/// whole text exactly once.
pub fn layout(
    text: &str,
    font: FontId,
    text_color: Color32,
    dim: Color32,
    wrap_width: f32,
) -> LayoutJob {
    let chars: Vec<char> = text.chars().collect();
    let flags = flags(&chars);

    let mut job = LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: wrap_width,
            ..Default::default()
        },
        ..Default::default()
    };
    if chars.is_empty() {
        job.append("", 0.0, format(0, &font, text_color, dim));
        return job;
    }
    // Chars with equal flags form one run, converted to a byte range by
    // slicing the original text at char boundaries.
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    let mut from = 0;
    for i in 1..=chars.len() {
        if i == chars.len() || flags[i] != flags[from] {
            let run = &text[offsets[from]..offsets[i]];
            job.append(run, 0.0, format(flags[from], &font, text_color, dim));
            from = i;
        }
    }
    job
}

fn format(flags: u8, font: &FontId, text_color: Color32, dim: Color32) -> TextFormat {
    let color = if flags & DIM != 0 { dim } else { text_color };
    let styled = flags & DIM == 0;
    let font_id = if flags & CODE != 0 {
        theme::mono(font.size)
    } else if styled && flags & BOLD != 0 {
        theme::semibold(font.size)
    } else {
        font.clone()
    };
    let line = Stroke::new(1.0, color);
    TextFormat {
        font_id,
        color,
        italics: styled && flags & ITALIC != 0,
        strikethrough: if styled && flags & STRIKE != 0 {
            line
        } else {
            Stroke::NONE
        },
        underline: if styled && flags & (UNDER | LINK) != 0 {
            line
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
}

/// One flag set per char.
fn flags(chars: &[char]) -> Vec<u8> {
    let mut flags = vec![0u8; chars.len()];
    let mut in_fence = false;
    let mut start = 0;
    while start <= chars.len() {
        let end = chars[start..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(chars.len(), |p| start + p);
        let line = &chars[start..end];
        let trimmed = line.iter().take_while(|c| c.is_whitespace()).count();
        if line[trimmed..].starts_with(&['`', '`', '`']) {
            in_fence = !in_fence;
            flags[start..end].fill(DIM | CODE);
        } else if in_fence {
            flags[start..end].fill(CODE);
        } else {
            line_flags(chars, &mut flags, start, end, trimmed);
        }
        start = end + 1;
    }
    flags
}

fn line_flags(chars: &[char], flags: &mut [u8], start: usize, end: usize, indent: usize) {
    let body = start + indent;
    let rest = &chars[body..end];
    let prefix = if rest.starts_with(&['>', ' ']) {
        2
    } else if rest == ['>'] {
        1
    } else if rest.starts_with(&['-', ' ']) || rest.starts_with(&['*', ' ']) {
        2
    } else {
        let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && rest[digits..].starts_with(&['.', ' ']) {
            digits + 2
        } else {
            0
        }
    };
    flags[body..body + prefix].fill(DIM);
    inline(chars, flags, body + prefix, end);
}

/// Code spans first (their content is literal), then emphasis in the text
/// between them.
fn inline(chars: &[char], flags: &mut [u8], start: usize, end: usize) {
    let mut at = start;
    while let Some(open) = chars[at..end]
        .iter()
        .position(|&c| c == '`')
        .map(|p| at + p)
    {
        let Some(close) = chars[open + 1..end]
            .iter()
            .position(|&c| c == '`')
            .map(|p| open + 1 + p)
        else {
            break;
        };
        emphasis(chars, flags, at, open);
        flags[open] = DIM;
        flags[open + 1..close].fill(CODE);
        flags[close] = DIM;
        at = close + 1;
    }
    emphasis(chars, flags, at, end);
}

fn emphasis(chars: &[char], flags: &mut [u8], start: usize, end: usize) {
    let seg = &chars[start..end];
    let mut protected = vec![false; seg.len()];
    link_flags(seg, &mut flags[start..end], &mut protected);
    for (marker, flag) in [
        ("**", BOLD),
        ("~~", STRIKE),
        ("++", UNDER),
        ("*", ITALIC),
        ("_", ITALIC),
    ] {
        let marker: Vec<char> = marker.chars().collect();
        wrap_pairs(seg, &mut flags[start..end], &mut protected, &marker, flag);
    }
}

/// Bare `http(s)://` words, without trailing punctuation.
fn link_flags(seg: &[char], flags: &mut [u8], protected: &mut [bool]) {
    let mut at = 0;
    for word in seg.split(|&c| c == ' ') {
        let kept = word
            .iter()
            .rposition(|c| !matches!(c, '.' | ',' | ')' | '!' | '?' | ';' | ':'))
            .map_or(0, |p| p + 1);
        let link = &word[..kept];
        let text: String = link.iter().collect();
        if (text.starts_with("https://") || text.starts_with("http://")) && link.len() > 8 {
            flags[at..at + kept].fill(LINK);
            protected[at..at + kept].fill(true);
        }
        at += word.len() + 1;
    }
}

fn wrap_pairs(seg: &[char], flags: &mut [u8], protected: &mut [bool], marker: &[char], flag: u8) {
    let m = marker.len();
    let mut at = 0;
    while let Some(open) = find_marker(seg, protected, marker, at, true) {
        // Something must sit between the markers.
        let Some(close) = find_marker(seg, protected, marker, open + m + 1, false) else {
            break;
        };
        for f in &mut flags[open + m..close] {
            *f |= flag;
        }
        for i in (open..open + m).chain(close..close + m) {
            flags[i] = DIM;
            protected[i] = true;
        }
        at = close + m;
    }
}

fn find_marker(
    seg: &[char],
    protected: &[bool],
    marker: &[char],
    from: usize,
    opening: bool,
) -> Option<usize> {
    let m = marker.len();
    for i in from..seg.len().saturating_sub(m - 1) {
        if i + m > seg.len() || seg[i..i + m] != *marker || protected[i..i + m].contains(&true) {
            continue;
        }
        let before = i.checked_sub(1).map(|p| seg[p]);
        let after = seg.get(i + m).copied();
        let ok = if opening {
            after.is_some_and(|c| !c.is_whitespace()) && before.is_none_or(|c| !c.is_alphanumeric())
        } else {
            before.is_some_and(|c| !c.is_whitespace()) && after.is_none_or(|c| !c.is_alphanumeric())
        };
        // An inner `*` of `**` is not a single marker.
        let doubled = m == 1 && (after == Some(marker[0]) || before == Some(marker[0]));
        if ok && !doubled {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: Color32 = Color32::WHITE;
    const DIMMED: Color32 = Color32::GRAY;

    fn job(text: &str) -> LayoutJob {
        layout(text, theme::regular(14.0), TEXT, DIMMED, 300.0)
    }

    /// Each section as (text, format).
    fn parts(job: &LayoutJob) -> Vec<(String, TextFormat)> {
        job.sections
            .iter()
            .map(|s| {
                (
                    job.text[s.byte_range.start.0..s.byte_range.end.0].to_owned(),
                    s.format.clone(),
                )
            })
            .collect()
    }

    fn find<'a>(parts: &'a [(String, TextFormat)], text: &str) -> &'a TextFormat {
        &parts
            .iter()
            .find(|(t, _)| t == text)
            .unwrap_or_else(|| panic!("no section {text:?} in {:?}", names(parts)))
            .1
    }

    fn names(parts: &[(String, TextFormat)]) -> Vec<&str> {
        parts.iter().map(|(t, _)| t.as_str()).collect()
    }

    #[test]
    fn plain_one_section() {
        assert_eq!(job("abc").sections.len(), 1);
        assert_eq!(job("").sections.len(), 1);
    }

    #[test]
    fn bold_section() {
        let parts = parts(&job("a **b** c"));
        assert_eq!(names(&parts), ["a ", "**", "b", "**", " c"]);
        assert_eq!(find(&parts, "b").font_id, theme::semibold(14.0));
        assert_eq!(find(&parts, "**").color, DIMMED);
        assert_eq!(find(&parts, "b").color, TEXT);
    }

    #[test]
    fn italic_strike_underline() {
        let p = parts(&job("*i* ~~s~~ ++u++ _j_"));
        assert!(find(&p, "i").italics);
        assert_ne!(find(&p, "s").strikethrough, Stroke::NONE);
        assert_ne!(find(&p, "u").underline, Stroke::NONE);
        assert!(find(&p, "j").italics);
        assert_eq!(find(&p, "++").color, DIMMED);
    }

    #[test]
    fn code_inline() {
        let p = parts(&job("x `a **b**` y"));
        assert_eq!(find(&p, "a **b**").font_id, theme::mono(14.0));
        // Emphasis inside code is literal.
        assert!(!names(&p).contains(&"b"));
    }

    #[test]
    fn link_underlined() {
        let p = parts(&job("see https://x.io."));
        assert_ne!(find(&p, "https://x.io").underline, Stroke::NONE);
        assert_eq!(find(&p, "see ").underline, Stroke::NONE);
    }

    #[test]
    fn link_is_not_emphasis() {
        let p = parts(&job("https://x.io/a_b_c"));
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn quote_line() {
        let p = parts(&job("> q"));
        assert_eq!(names(&p), ["> ", "q"]);
        assert_eq!(find(&p, "> ").color, DIMMED);
    }

    #[test]
    fn list_markers_dim() {
        let p = parts(&job("- a\n12. b"));
        assert_eq!(find(&p, "- ").color, DIMMED);
        assert_eq!(find(&p, "12. ").color, DIMMED);
    }

    #[test]
    fn fence() {
        let p = parts(&job("a\n```\nlet **x**\n```\nb **c**"));
        assert_eq!(find(&p, "let **x**").font_id, theme::mono(14.0));
        assert_eq!(find(&p, "c").font_id, theme::semibold(14.0));
        assert_eq!(find(&p, "a\n").font_id, theme::regular(14.0));
    }

    #[test]
    fn unclosed_markers() {
        for text in [
            "**a",
            "*a",
            "`a",
            "a ~~b",
            "++a",
            "a * b * c",
            "snake_case_name",
        ] {
            assert_eq!(job(text).sections.len(), 1, "{text}");
        }
    }

    #[test]
    fn byte_ranges() {
        for text in [
            "é **b** ✓",
            "日本 `✓` _é_ ++ü++\n> 🙂 ~~x~~\n```\né\n```",
            "",
            "**日本**",
        ] {
            let job = job(text);
            assert_eq!(job.text, text);
            let mut next = 0;
            for s in &job.sections {
                assert_eq!(s.byte_range.start.0, next, "{text}");
                assert!(text.is_char_boundary(s.byte_range.end.0));
                next = s.byte_range.end.0;
            }
            assert_eq!(next, text.len(), "{text}");
        }
    }

    #[test]
    fn wrap_width_is_kept() {
        assert_eq!(job("a").wrap.max_width, 300.0);
    }
}
