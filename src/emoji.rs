//! Emoji lookup for the picker and for typing: search by name or shortcode,
//! `:shortcode:` and emoticon conversion before the cursor.
//!
//! Built on the `emojis` crate (CLDR names, groups, GitHub shortcodes).
//! Emoji newer than Unicode 15.0 are left out because the bundled fonts do
//! not draw them. All indices are char indices, as in egui's `CCursor`.

use std::ops::Range;

use emojis::{Emoji, Group, UnicodeVersion};

/// Newest Unicode version the picker offers.
const NEWEST: UnicodeVersion = UnicodeVersion::new(15, 0);

/// Emoticons and what they turn into, longest first.
const EMOTICONS: &[(&str, &str)] = &[
    (":-)", "\u{1F642}"),
    (":-(", "\u{1F641}"),
    (":-D", "\u{1F603}"),
    (";-)", "\u{1F609}"),
    (":-P", "\u{1F61B}"),
    (":-p", "\u{1F61B}"),
    (":)", "\u{1F642}"),
    (":(", "\u{1F641}"),
    (":D", "\u{1F603}"),
    (";)", "\u{1F609}"),
    (":P", "\u{1F61B}"),
    (":p", "\u{1F61B}"),
    ("<3", "\u{2764}\u{FE0F}"),
];

/// Whether the picker and search may offer this emoji: no skin tone
/// variants (the base emoji stands for them) and nothing too new.
fn offered(emoji: &Emoji) -> bool {
    emoji.unicode_version() <= NEWEST
        && emoji
            .skin_tone()
            .is_none_or(|tone| tone == emojis::SkinTone::Default)
}

/// Emoji matching `query` in their name or shortcode, best matches first,
/// at most `limit`. An empty query matches nothing.
pub fn search(query: &str, limit: usize) -> Vec<&'static Emoji> {
    let query = query.trim().trim_matches(':').to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, &'static Emoji)> = emojis::iter()
        .filter(|emoji| offered(emoji))
        .filter_map(|emoji| rank(emoji, &query).map(|rank| (rank, emoji)))
        .collect();
    // Stable: equal ranks keep the CLDR order (thumbs up before down).
    hits.sort_by_key(|(rank, _)| *rank);
    hits.into_iter()
        .take(limit)
        .map(|(_, emoji)| emoji)
        .collect()
}

fn rank(emoji: &Emoji, query: &str) -> Option<u8> {
    let name = emoji.name().to_lowercase();
    let mut best: Option<u8> = None;
    let mut note = |rank: u8| best = Some(best.map_or(rank, |b| b.min(rank)));
    for code in emoji.shortcodes() {
        if code == query {
            note(0);
        } else if code.starts_with(query) {
            note(1);
        } else if code.contains(query) {
            note(4);
        }
    }
    if name == query {
        note(0);
    } else if name.starts_with(query) {
        note(2);
    } else if name.split(' ').any(|word| word.starts_with(query)) {
        note(3);
    } else if name.contains(query) {
        note(4);
    }
    best
}

/// The picker's groups in display order, each with its emoji.
pub fn groups() -> Vec<(&'static str, Vec<&'static Emoji>)> {
    Group::iter()
        .map(|group| {
            let name = match group {
                Group::SmileysAndEmotion => "Smileys & Emotion",
                Group::PeopleAndBody => "People & Body",
                Group::AnimalsAndNature => "Animals & Nature",
                Group::FoodAndDrink => "Food & Drink",
                Group::TravelAndPlaces => "Travel & Places",
                Group::Activities => "Activities",
                Group::Objects => "Objects",
                Group::Symbols => "Symbols",
                Group::Flags => "Flags",
            };
            let emoji = group.emojis().filter(|e| offered(e)).collect();
            (name, emoji)
        })
        .collect()
}

/// A `:shortcode:` or emoticon that ends at `cursor` (or, for an emoticon,
/// just before a space at `cursor - 1`), with the emoji that replaces it.
/// The range is in chars and never includes the space.
///
/// Nothing converts inside code spans or fences, inside links, or in the
/// middle of a word (`a:)`).
pub fn convert_before_cursor(text: &str, cursor: usize) -> Option<(Range<usize>, &'static str)> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    if cursor == 0 {
        return None;
    }
    shortcode_before(&chars, cursor).or_else(|| {
        let end = if chars[cursor - 1] == ' ' {
            cursor - 1
        } else {
            cursor
        };
        emoticon_before(&chars, end)
    })
}

fn shortcode_before(chars: &[char], cursor: usize) -> Option<(Range<usize>, &'static str)> {
    if chars[cursor - 1] != ':' {
        return None;
    }
    let is_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-');
    let mut open = cursor - 1;
    while open > 0 && is_name(chars[open - 1]) {
        open -= 1;
    }
    // `open` is the first name char; the colon sits before it.
    if open == 0 || chars[open - 1] != ':' || open == cursor - 1 {
        return None;
    }
    let colon = open - 1;
    if colon > 0 && chars[colon - 1].is_alphanumeric() {
        return None;
    }
    if protected(chars, colon) {
        return None;
    }
    let name: String = chars[open..cursor - 1].iter().collect();
    let emoji = emojis::get_by_shortcode(&name.to_lowercase())?;
    offered(emoji).then(|| (colon..cursor, emoji.as_str()))
}

fn emoticon_before(chars: &[char], end: usize) -> Option<(Range<usize>, &'static str)> {
    for (emoticon, emoji) in EMOTICONS {
        let len = emoticon.chars().count();
        if end < len || !chars[end - len..end].iter().copied().eq(emoticon.chars()) {
            continue;
        }
        let start = end - len;
        if start > 0 && !chars[start - 1].is_whitespace() {
            continue;
        }
        if protected(chars, start) {
            continue;
        }
        return Some((start..end, emoji));
    }
    None
}

/// Whether `at` sits in a code span, a fence or a link.
fn protected(chars: &[char], at: usize) -> bool {
    let mut in_fence = false;
    let mut in_span = false;
    let mut line_start = 0;
    let mut i = 0;
    while i < at {
        if i == line_start {
            // A fence line toggles the block; its text is never converted.
            let indent = chars[i..].iter().take_while(|c| **c == ' ').count();
            if chars[i + indent..].starts_with(&['`', '`', '`']) {
                in_fence = !in_fence;
                in_span = false;
                // Skip the rest of the fence line.
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                if i >= at {
                    return true;
                }
                line_start = i + 1;
                i += 1;
                continue;
            }
        }
        match chars[i] {
            '\n' => {
                line_start = i + 1;
                in_span = false;
            }
            '`' if !in_fence => in_span = !in_span,
            _ => {}
        }
        i += 1;
    }
    if in_fence || in_span {
        return true;
    }
    // The word holding `at` starts a link.
    let word_start = chars[..at]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |p| p + 1);
    let head: String = chars[word_start..at.min(word_start + 8)].iter().collect();
    let head = head.to_lowercase();
    ["http://", "https://", "www."]
        .iter()
        .any(|scheme| head.starts_with(scheme))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcode_converts() {
        assert_eq!(
            convert_before_cursor("hi :smile:", 10),
            Some((3..10, "\u{1F604}"))
        );
    }

    #[test]
    fn shortcode_multibyte_prefix_uses_char_indices() {
        // "é✓ " is three chars but five bytes.
        let text = "é✓ :smile:";
        assert_eq!(convert_before_cursor(text, 10), Some((3..10, "\u{1F604}")));
    }

    #[test]
    fn emoticon_after_space() {
        assert_eq!(
            convert_before_cursor("ok :) ", 6),
            Some((3..5, "\u{1F642}"))
        );
        // At the end, without the space, as on send.
        assert_eq!(convert_before_cursor("ok :)", 5), Some((3..5, "\u{1F642}")));
    }

    #[test]
    fn search_thumbs() {
        let hits = search("thumbs", 3);
        assert_eq!(hits.first().map(|e| e.as_str()), Some("\u{1F44D}"));
        assert!(hits.len() <= 3);
    }

    #[test]
    fn search_empty_and_limit() {
        assert!(search("", 5).is_empty());
        assert!(search("heart", 4).len() <= 4);
    }

    #[test]
    fn unknown_shortcode() {
        assert_eq!(convert_before_cursor(":nope:", 6), None);
    }

    #[test]
    fn emoticons() {
        for (emoticon, emoji) in EMOTICONS {
            assert!(emojis::get(emoji).is_some(), "{emoticon} maps to an emoji");
            let text = format!("x {emoticon} ");
            let cursor = text.chars().count();
            let (range, got) = convert_before_cursor(&text, cursor).unwrap();
            assert_eq!(got, *emoji);
            assert_eq!(range, 2..2 + emoticon.chars().count());
        }
    }

    #[test]
    fn url_not_emoticon() {
        assert_eq!(convert_before_cursor("http://x", 8), None);
        assert_eq!(convert_before_cursor("see https://a.io:D ", 19), None);
        assert_eq!(convert_before_cursor("https://x.io/:smile:", 20), None);
    }

    #[test]
    fn code_untouched() {
        assert_eq!(convert_before_cursor("`a :smile:", 10), None);
        assert_eq!(convert_before_cursor("`a :) ", 6), None);
        assert_eq!(convert_before_cursor("```\n:smile:", 11), None);
        // After the span closed, conversion works again.
        assert!(convert_before_cursor("`a` :smile:", 11).is_some());
        // The span does not run past a line break.
        assert!(convert_before_cursor("`a\n:smile:", 10).is_some());
    }

    #[test]
    fn no_mid_word() {
        assert_eq!(convert_before_cursor("a:)", 3), None);
        assert_eq!(convert_before_cursor("a:smile:", 8), None);
        assert_eq!(convert_before_cursor("12:30:", 6), None);
    }

    #[test]
    fn groups_nonempty() {
        let groups = groups();
        assert_eq!(groups[0].0, "Smileys & Emotion");
        assert!(groups.iter().all(|(_, emoji)| !emoji.is_empty()));
        assert!(
            groups
                .iter()
                .flat_map(|(_, emoji)| emoji)
                .all(|e| e.unicode_version() <= NEWEST)
        );
    }

    #[test]
    fn cursor_past_end_is_clamped() {
        assert_eq!(convert_before_cursor(":)", 99), Some((0..2, "\u{1F642}")));
        assert_eq!(convert_before_cursor("", 0), None);
    }
}
