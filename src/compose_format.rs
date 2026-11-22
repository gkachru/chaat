//! Formatting edits on the composer's draft text: toolbar buttons and
//! shortcuts. Pure text in, text and selection out.
//!
//! All ranges are char indices (egui's `CCursor`), never byte offsets, so
//! multibyte text edits at the right place. The markers are the ones
//! `html::compose` understands, plus `++underline++`.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    CodeBlock,
    Quote,
    Bullet,
    Numbered,
}

/// The format a shortcut asks for, if any: Ctrl/Cmd+B, I, U and
/// Shift+Ctrl/Cmd+X.
pub fn shortcut(modifiers: egui::Modifiers, key: egui::Key) -> Option<Format> {
    if !modifiers.command || modifiers.alt {
        return None;
    }
    match (modifiers.shift, key) {
        (false, egui::Key::B) => Some(Format::Bold),
        (false, egui::Key::I) => Some(Format::Italic),
        (false, egui::Key::U) => Some(Format::Underline),
        (true, egui::Key::X) => Some(Format::Strike),
        _ => None,
    }
}

/// Applies `format` to `selection` (char indices) and returns the new text
/// and the selection to show afterwards. Applying the same format twice
/// undoes it.
pub fn apply(text: &str, selection: Range<usize>, format: Format) -> (String, Range<usize>) {
    let chars: Vec<char> = text.chars().collect();
    let end = selection.end.min(chars.len());
    let start = selection.start.min(end);
    match format {
        Format::Bold => inline(&chars, start, end, "**"),
        Format::Italic => inline(&chars, start, end, "*"),
        Format::Underline => inline(&chars, start, end, "++"),
        Format::Strike => inline(&chars, start, end, "~~"),
        Format::Code => inline(&chars, start, end, "`"),
        Format::CodeBlock => code_block(&chars, start, end),
        Format::Quote => prefix_lines(&chars, start, end, Prefix::Fixed("> ")),
        Format::Bullet => prefix_lines(&chars, start, end, Prefix::Fixed("- ")),
        Format::Numbered => prefix_lines(&chars, start, end, Prefix::Numbered),
    }
}

fn collect(chars: &[char]) -> String {
    chars.iter().collect()
}

fn matches_at(chars: &[char], at: usize, marker: &[char]) -> bool {
    chars.get(at..at + marker.len()) == Some(marker)
}

fn inline(chars: &[char], start: usize, end: usize, marker: &str) -> (String, Range<usize>) {
    let marker: Vec<char> = marker.chars().collect();
    let m = marker.len();
    // A single `*` next to another `*` belongs to bold, not italic.
    let star = marker == ['*'];
    let is_star = |at: Option<usize>| at.is_some_and(|i| chars.get(i) == Some(&'*'));

    // The selection includes its own markers: strip them.
    if end - start >= 2 * m
        && matches_at(chars, start, &marker)
        && matches_at(chars, end - m, &marker)
        && !(star && (is_star(Some(start + 1)) || is_star(Some(end - 2))))
    {
        let mut out = chars[..start].to_vec();
        out.extend_from_slice(&chars[start + m..end - m]);
        out.extend_from_slice(&chars[end..]);
        return (collect(&out), start..end - 2 * m);
    }
    // The markers sit just outside the selection: strip them.
    if start >= m
        && matches_at(chars, start - m, &marker)
        && matches_at(chars, end, &marker)
        && !(star && (is_star(start.checked_sub(2)) || is_star(Some(end + 1))))
    {
        let mut out = chars[..start - m].to_vec();
        out.extend_from_slice(&chars[start..end]);
        out.extend_from_slice(&chars[end + m..]);
        return (collect(&out), start - m..end - m);
    }
    let mut out = chars[..start].to_vec();
    out.extend_from_slice(&marker);
    out.extend_from_slice(&chars[start..end]);
    out.extend_from_slice(&marker);
    out.extend_from_slice(&chars[end..]);
    (collect(&out), start + m..end + m)
}

/// Start of the line holding `at`, and the end of the line holding `at`.
fn line_start(chars: &[char], at: usize) -> usize {
    chars[..at]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |i| i + 1)
}

fn line_end(chars: &[char], at: usize) -> usize {
    chars[at..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(chars.len(), |i| at + i)
}

fn code_block(chars: &[char], start: usize, end: usize) -> (String, Range<usize>) {
    let first = line_start(chars, start);
    // A selection that ends right after a newline does not take the next line.
    let last = if end > start && chars.get(end - 1) == Some(&'\n') {
        end - 1
    } else {
        line_end(chars, end)
    };
    let fence: Vec<char> = "```".chars().collect();

    // Already fenced: the lines around the block are fences.
    if first >= 4 && last < chars.len() {
        let above_end = first - 1;
        let above_start = line_start(chars, above_end);
        let below_start = last + 1;
        let below_end = line_end(chars, below_start.min(chars.len()));
        if below_start <= chars.len()
            && chars[above_start..above_end] == fence[..]
            && chars[below_start..below_end] == fence[..]
        {
            let mut out = chars[..above_start].to_vec();
            out.extend_from_slice(&chars[first..last]);
            out.extend_from_slice(&chars[below_end..]);
            let n = above_start;
            return (collect(&out), n..n + (last - first));
        }
    }
    let mut out = chars[..first].to_vec();
    out.extend_from_slice(&fence);
    out.push('\n');
    out.extend_from_slice(&chars[first..last]);
    out.push('\n');
    out.extend_from_slice(&fence);
    out.extend_from_slice(&chars[last..]);
    let inner = first + 4;
    (collect(&out), inner..inner + (last - first))
}

/// What a new line at `cursor` (chars) does in a list item (`- `, `* `,
/// `1. `): it starts the next item, numbered one up, or, on an item with no
/// text, removes the marker to end the list. The new text and caret, or
/// `None` to let the editor insert a plain new line.
pub fn continue_list(text: &str, cursor: usize) -> Option<(String, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let start = line_start(&chars, cursor);
    let end = line_end(&chars, cursor);
    let line = &chars[start..end];
    let (marker_len, next) = if line.starts_with(&['-', ' ']) || line.starts_with(&['*', ' ']) {
        (2, format!("{} ", line[0]))
    } else {
        let n = numbered_len(line)?;
        let number: u64 = line[..n - 2].iter().collect::<String>().parse().ok()?;
        (n, format!("{}. ", number + 1))
    };
    if cursor < start + marker_len {
        return None;
    }
    if line.len() == marker_len {
        let mut out = chars[..start].to_vec();
        out.extend_from_slice(&chars[end..]);
        return Some((collect(&out), start));
    }
    let mut out = chars[..cursor].to_vec();
    out.push('\n');
    out.extend(next.chars());
    let caret = out.len();
    out.extend_from_slice(&chars[cursor..]);
    Some((collect(&out), caret))
}

#[derive(Clone, Copy)]
enum Prefix {
    Fixed(&'static str),
    Numbered,
}

/// Length in chars of a numbered prefix (`12. `) at the start of `line`.
fn numbered_len(line: &[char]) -> Option<usize> {
    let digits = line.iter().take_while(|c| c.is_ascii_digit()).count();
    (digits > 0 && line.get(digits) == Some(&'.') && line.get(digits + 1) == Some(&' '))
        .then_some(digits + 2)
}

fn prefix_len(line: &[char], prefix: Prefix) -> Option<usize> {
    match prefix {
        Prefix::Fixed(p) => {
            let p: Vec<char> = p.chars().collect();
            line.starts_with(&p).then_some(p.len())
        }
        Prefix::Numbered => numbered_len(line),
    }
}

fn prefix_lines(
    chars: &[char],
    start: usize,
    end: usize,
    prefix: Prefix,
) -> (String, Range<usize>) {
    let first = line_start(chars, start);
    let last = if end > start && chars.get(end - 1) == Some(&'\n') {
        end - 1
    } else {
        line_end(chars, end)
    };
    let region: String = chars[first..last].iter().collect();
    let lines: Vec<Vec<char>> = region.split('\n').map(|l| l.chars().collect()).collect();
    let single = lines.len() == 1;
    // Blank lines are left alone in a multi-line selection.
    let targets = |l: &&Vec<char>| single || !l.is_empty();
    let all_prefixed = lines
        .iter()
        .filter(targets)
        .all(|l| prefix_len(l, prefix).is_some());

    let mut number = 0;
    let rebuilt: Vec<String> = lines
        .iter()
        .map(|line| {
            if !targets(&line) {
                return String::new();
            }
            if all_prefixed {
                let n = prefix_len(line, prefix).unwrap_or(0);
                line[n..].iter().collect()
            } else {
                number += 1;
                let head = match prefix {
                    Prefix::Fixed(p) => p.to_owned(),
                    Prefix::Numbered => format!("{number}. "),
                };
                format!("{head}{}", line.iter().collect::<String>())
            }
        })
        .collect();
    let body = rebuilt.join("\n");
    let body_len = body.chars().count();
    let mut out = chars[..first].to_vec();
    out.extend(body.chars());
    out.extend_from_slice(&chars[last..]);
    (collect(&out), first..first + body_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_wraps() {
        assert_eq!(
            apply("hi there", 3..8, Format::Bold),
            ("hi **there**".to_owned(), 5..10)
        );
    }

    #[test]
    fn toggle_off() {
        assert_eq!(apply("**a**", 2..3, Format::Bold), ("a".to_owned(), 0..1));
        // Selection that includes the markers.
        assert_eq!(apply("**a**", 0..5, Format::Bold), ("a".to_owned(), 0..1));
    }

    #[test]
    fn italic_inside_bold_wraps() {
        let (text, range) = apply("**a**", 2..3, Format::Italic);
        assert_eq!(text, "***a***");
        assert_eq!(range, 3..4);
    }

    #[test]
    fn multibyte() {
        assert_eq!(
            apply("é x", 0..1, Format::Italic),
            ("*é* x".to_owned(), 1..2)
        );
        assert_eq!(
            apply("日本 ✓ ok", 3..4, Format::Bold),
            ("日本 **✓** ok".to_owned(), 5..6)
        );
    }

    #[test]
    fn underline_uses_plus() {
        assert_eq!(apply("a", 0..1, Format::Underline).0, "++a++");
        assert_eq!(apply("a", 0..1, Format::Strike).0, "~~a~~");
        assert_eq!(apply("a", 0..1, Format::Code).0, "`a`");
    }

    #[test]
    fn code_block() {
        let (text, range) = apply("a\nb", 0..3, Format::CodeBlock);
        assert_eq!(text, "```\na\nb\n```");
        assert_eq!(range, 4..7);
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars[range.clone()].iter().collect::<String>(), "a\nb");
        // Again removes the fences.
        assert_eq!(
            apply(&text, range, Format::CodeBlock),
            ("a\nb".to_owned(), 0..3)
        );
    }

    #[test]
    fn code_block_keeps_surroundings() {
        let (text, _) = apply("x\nyy\nz", 2..4, Format::CodeBlock);
        assert_eq!(text, "x\n```\nyy\n```\nz");
    }

    #[test]
    fn bullets() {
        let (text, range) = apply("a\nb", 0..3, Format::Bullet);
        assert_eq!((text.as_str(), range.clone()), ("- a\n- b", 0..7));
        assert_eq!(
            apply(&text, range, Format::Bullet),
            ("a\nb".to_owned(), 0..3)
        );
    }

    #[test]
    fn numbered() {
        let (text, range) = apply("a\nb", 0..3, Format::Numbered);
        assert_eq!(text, "1. a\n2. b");
        assert_eq!(
            apply(&text, range, Format::Numbered),
            ("a\nb".to_owned(), 0..3)
        );
    }

    #[test]
    fn a_new_line_in_a_list_starts_the_next_item() {
        assert_eq!(continue_list("- a", 3), Some(("- a\n- ".to_owned(), 6)));
        assert_eq!(continue_list("* a", 3), Some(("* a\n* ".to_owned(), 6)));
        assert_eq!(
            continue_list("1. a\n2. b", 9),
            Some(("1. a\n2. b\n3. ".to_owned(), 13))
        );
        // Inside an item: the rest moves to the new item.
        assert_eq!(continue_list("- ab", 3), Some(("- a\n- b".to_owned(), 6)));
        assert_eq!(
            continue_list("x\n- é", 5),
            Some(("x\n- é\n- ".to_owned(), 8))
        );
    }

    #[test]
    fn a_new_line_on_an_empty_item_ends_the_list() {
        assert_eq!(continue_list("- a\n- ", 6), Some(("- a\n".to_owned(), 4)));
        assert_eq!(
            continue_list("1. a\n2. ", 8),
            Some(("1. a\n".to_owned(), 5))
        );
    }

    #[test]
    fn a_new_line_outside_a_list_is_left_to_the_editor() {
        assert_eq!(continue_list("abc", 3), None);
        assert_eq!(continue_list("> q", 3), None);
        assert_eq!(continue_list("-a", 2), None);
        assert_eq!(continue_list("12.x", 4), None);
        // The caret before the marker's end: a plain new line.
        assert_eq!(continue_list("- a", 1), None);
        assert_eq!(continue_list("", 0), None);
        assert_eq!(continue_list("- a", 9), Some(("- a\n- ".to_owned(), 6)));
    }

    #[test]
    fn quote() {
        assert_eq!(apply("a", 0..1, Format::Quote), ("> a".to_owned(), 0..3));
        assert_eq!(apply("x\ny", 2..3, Format::Quote).0, "x\n> y");
    }

    #[test]
    fn line_format_from_mid_line_multibyte() {
        assert_eq!(
            apply("é\nü", 1..3, Format::Bullet),
            ("- é\n- ü".to_owned(), 0..7)
        );
    }

    #[test]
    fn empty_selection() {
        assert_eq!(apply("", 0..0, Format::Bold), ("****".to_owned(), 2..2));
        assert_eq!(apply("", 0..0, Format::Bullet), ("- ".to_owned(), 0..2));
        assert_eq!(
            apply("", 0..0, Format::CodeBlock),
            ("```\n\n```".to_owned(), 4..4)
        );
    }

    #[test]
    fn out_of_range_selection_is_clamped() {
        assert_eq!(apply("ab", 1..9, Format::Bold).0, "a**b**");
    }

    #[test]
    fn shortcut_map() {
        use egui::{Key, Modifiers};
        let cmd = Modifiers::COMMAND;
        assert_eq!(shortcut(cmd, Key::B), Some(Format::Bold));
        assert_eq!(shortcut(cmd, Key::I), Some(Format::Italic));
        assert_eq!(shortcut(cmd, Key::U), Some(Format::Underline));
        let shifted = Modifiers {
            shift: true,
            ..Modifiers::COMMAND
        };
        assert_eq!(shortcut(shifted, Key::X), Some(Format::Strike));
        assert_eq!(shortcut(cmd, Key::X), None);
        assert_eq!(shortcut(Modifiers::NONE, Key::B), None);
        assert_eq!(shortcut(cmd, Key::K), None);
    }
}
