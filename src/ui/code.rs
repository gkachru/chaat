//! Code blocks in messages: a small tokenizer for syntax colours and the
//! block widget with a Copy button.
//!
//! The tokenizer knows keywords, strings, numbers and comments for the
//! languages people paste into chats. It is not a parser: it never fails,
//! and for an unknown language it colours only strings, numbers and comments.

use std::ops::Range;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, Margin, Stroke, Ui};

use crate::model::Action;
use crate::theme::{self, Icon, Palette};

use super::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Keyword,
    String,
    Number,
    Comment,
}

/// What differs between languages.
struct Lang {
    keywords: &'static [&'static str],
    /// Keywords match without regard to case.
    ignore_case: bool,
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// Quote characters that open a string.
    quotes: &'static str,
    /// `'` opens a character literal only when it closes at once (Rust).
    char_literals: bool,
    triple_quotes: bool,
}

const C_LIKE: Lang = Lang {
    keywords: &[],
    ignore_case: false,
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    quotes: "\"'",
    char_literals: false,
    triple_quotes: false,
};

const UNKNOWN: Lang = Lang {
    keywords: &[],
    ignore_case: false,
    line_comments: &["//", "#"],
    block_comment: Some(("/*", "*/")),
    quotes: "\"'",
    char_literals: false,
    triple_quotes: false,
};

fn language(name: Option<&str>) -> Lang {
    let name = name.unwrap_or_default().trim().to_ascii_lowercase();
    match name.as_str() {
        "rust" | "rs" => Lang {
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
                "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
                "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            char_literals: true,
            ..C_LIKE
        },
        "python" | "py" => Lang {
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def",
                "del", "elif", "else", "except", "False", "finally", "for", "from", "global", "if",
                "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise",
                "return", "True", "try", "while", "with", "yield",
            ],
            line_comments: &["#"],
            block_comment: None,
            triple_quotes: true,
            ..C_LIKE
        },
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" => Lang {
            keywords: &[
                "async",
                "await",
                "break",
                "case",
                "catch",
                "class",
                "const",
                "continue",
                "default",
                "delete",
                "else",
                "export",
                "extends",
                "false",
                "finally",
                "for",
                "function",
                "if",
                "import",
                "in",
                "instanceof",
                "interface",
                "let",
                "new",
                "null",
                "of",
                "return",
                "static",
                "super",
                "switch",
                "this",
                "throw",
                "true",
                "try",
                "type",
                "typeof",
                "undefined",
                "var",
                "void",
                "while",
                "yield",
            ],
            quotes: "\"'`",
            ..C_LIKE
        },
        "c" | "cpp" | "c++" | "cc" | "h" | "hpp" | "cs" | "csharp" | "c#" | "java" | "go"
        | "golang" => Lang {
            keywords: &[
                "bool",
                "break",
                "case",
                "catch",
                "char",
                "class",
                "const",
                "continue",
                "default",
                "defer",
                "do",
                "double",
                "else",
                "enum",
                "false",
                "final",
                "float",
                "for",
                "func",
                "go",
                "if",
                "import",
                "int",
                "interface",
                "long",
                "namespace",
                "new",
                "nil",
                "null",
                "package",
                "private",
                "public",
                "range",
                "return",
                "static",
                "string",
                "struct",
                "switch",
                "this",
                "throw",
                "true",
                "try",
                "type",
                "using",
                "var",
                "void",
                "while",
            ],
            ..C_LIKE
        },
        "sql" => Lang {
            keywords: &[
                "add", "all", "alter", "and", "as", "asc", "between", "by", "case", "create",
                "delete", "desc", "distinct", "drop", "else", "end", "exists", "from", "group",
                "having", "in", "index", "inner", "insert", "into", "is", "join", "left", "like",
                "limit", "not", "null", "on", "or", "order", "outer", "right", "select", "set",
                "table", "then", "union", "update", "values", "view", "when", "where", "with",
            ],
            ignore_case: true,
            line_comments: &["--"],
            ..C_LIKE
        },
        "json" | "jsonc" => Lang {
            keywords: &["true", "false", "null"],
            line_comments: &[],
            block_comment: None,
            quotes: "\"",
            ..C_LIKE
        },
        "bash" | "sh" | "shell" | "zsh" => Lang {
            keywords: &[
                "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function",
                "if", "in", "local", "return", "select", "then", "until", "while",
            ],
            line_comments: &["#"],
            block_comment: None,
            ..C_LIKE
        },
        "powershell" | "ps1" | "pwsh" | "ps" => Lang {
            keywords: &[
                "begin", "break", "catch", "class", "continue", "do", "else", "elseif", "end",
                "finally", "for", "foreach", "function", "if", "in", "param", "process", "return",
                "switch", "throw", "try", "until", "while",
            ],
            ignore_case: true,
            line_comments: &["#"],
            block_comment: Some(("<#", "#>")),
            ..C_LIKE
        },
        _ => UNKNOWN,
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The colour classes of `code`, as byte ranges in order. Text between the
/// ranges is plain.
pub fn tokens(code: &str, language_name: Option<&str>) -> Vec<(Range<usize>, Token)> {
    let lang = language(language_name);
    let mut out = Vec::new();
    let mut i = 0;
    while i < code.len() {
        let rest = &code[i..];
        let Some(c) = rest.chars().next() else { break };
        // Comments.
        if let Some(marker) = lang.line_comments.iter().find(|m| rest.starts_with(**m)) {
            let end = line_end(code, i + marker.len());
            out.push((i..end, Token::Comment));
            i = end;
            continue;
        }
        if let Some((open, close)) = lang.block_comment
            && rest.starts_with(open)
        {
            let end = code[i + open.len()..]
                .find(close)
                .map_or(code.len(), |at| i + open.len() + at + close.len());
            out.push((i..end, Token::Comment));
            i = end;
            continue;
        }
        // Strings.
        if lang.quotes.contains(c) {
            let end = string_end(code, i, c, &lang);
            if end > i {
                out.push((i..end, Token::String));
                i = end;
                continue;
            }
        }
        // Words and numbers.
        if is_word(c) {
            let len = rest
                .char_indices()
                .find(|(_, ch)| !is_word(*ch))
                .map_or(rest.len(), |(at, _)| at);
            let word = &rest[..len];
            if c.is_ascii_digit() {
                let mut end = i + len;
                // A fraction or exponent sign continues the number.
                if code[end..].starts_with('.')
                    && code[end + 1..].starts_with(|d: char| d.is_ascii_digit())
                {
                    end += 1;
                    end += code[end..]
                        .find(|ch: char| !is_word(ch))
                        .unwrap_or(code.len() - end);
                }
                out.push((i..end, Token::Number));
                i = end;
                continue;
            }
            let hit = lang.keywords.iter().any(|k| {
                if lang.ignore_case {
                    k.eq_ignore_ascii_case(word)
                } else {
                    *k == word
                }
            });
            if hit {
                out.push((i..i + len, Token::Keyword));
            }
            i += len;
            continue;
        }
        i += c.len_utf8();
    }
    out
}

/// The end of the line that holds `from`, before its newline.
fn line_end(code: &str, from: usize) -> usize {
    code[from..].find('\n').map_or(code.len(), |at| from + at)
}

/// The end of the string opened by `quote` at `start`: after the closing
/// quote, or at the end of the line (of the text, for multi-line kinds).
/// Zero when the quote opens no string.
fn string_end(code: &str, start: usize, quote: char, lang: &Lang) -> usize {
    let body = start + quote.len_utf8();
    if quote == '\'' && lang.char_literals {
        // 'a', '\n': a character literal; anything else is a lifetime.
        let mut chars = code[body..].chars();
        let literal = match chars.next() {
            Some('\\') => chars.next().is_some(),
            Some(c) if c != '\'' => true,
            _ => false,
        };
        let close = body
            + code[body..]
                .chars()
                .take(if code[body..].starts_with('\\') { 2 } else { 1 })
                .map(char::len_utf8)
                .sum::<usize>();
        return if literal && code[close..].starts_with('\'') {
            close + 1
        } else {
            0
        };
    }
    let triple: String = std::iter::repeat_n(quote, 3).collect();
    if lang.triple_quotes && code[start..].starts_with(&triple) {
        return code[start + 3..]
            .find(&triple)
            .map_or(code.len(), |at| start + 3 + at + 3);
    }
    let multiline = quote == '`';
    let mut escaped = false;
    for (offset, c) in code[body..].char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return body + offset + c.len_utf8();
        } else if c == '\n' && !multiline {
            return body + offset;
        }
    }
    code.len()
}

fn color(token: Token, palette: &Palette) -> Color32 {
    match token {
        Token::Keyword => palette.accent,
        Token::String => palette.success,
        Token::Number => palette.warning,
        Token::Comment => palette.dim,
    }
}

/// The label over a code block: a language's display name, the name as
/// written when it is not known, "Code" when there is none.
pub fn display_name(language: Option<&str>) -> String {
    let written = language.map(str::trim).filter(|l| !l.is_empty());
    let Some(written) = written else {
        return "Code".to_owned();
    };
    match written.to_ascii_lowercase().as_str() {
        "rust" | "rs" => "Rust",
        "js" | "javascript" | "jsx" => "JavaScript",
        "ts" | "typescript" | "tsx" => "TypeScript",
        "py" | "python" => "Python",
        "cs" | "csharp" | "c#" => "C#",
        "cpp" | "c++" | "cc" | "hpp" => "C++",
        "c" | "h" => "C",
        "sql" => "SQL",
        "json" | "jsonc" => "JSON",
        "bash" | "sh" | "shell" | "zsh" => "Bash",
        "powershell" | "ps1" | "pwsh" | "ps" => "PowerShell",
        "go" | "golang" => "Go",
        "java" => "Java",
        _ => return written.to_owned(),
    }
    .to_owned()
}

fn layout(code: &str, language: Option<&str>, palette: &Palette) -> LayoutJob {
    let format = |color: Color32| TextFormat {
        font_id: theme::mono(12.0),
        color,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    let mut at = 0;
    for (range, token) in tokens(code, language) {
        if range.start > at {
            job.append(&code[at..range.start], 0.0, format(palette.text));
        }
        job.append(&code[range.clone()], 0.0, format(color(token, palette)));
        at = range.end;
    }
    if at < code.len() {
        job.append(&code[at..], 0.0, format(palette.text));
    }
    // Lines never wrap; a long one scrolls.
    job.wrap.max_width = f32::INFINITY;
    job
}

/// A code block: language label, Copy button and coloured, selectable text.
pub fn block(
    ui: &mut Ui,
    palette: &Palette,
    code: &str,
    language: Option<&str>,
    actions: &mut Vec<Action>,
) {
    copy_button(ui, palette, code, language, actions);
}

const FRAME_PAD: f32 = 10.0;
/// Narrowest a block gets: its label and Copy button fit.
const HEADER_MIN: f32 = 96.0;

/// Draws the block and returns its Copy button, for tests.
fn copy_button(
    ui: &mut Ui,
    palette: &Palette,
    code: &str,
    language: Option<&str>,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let label = display_name(language);
    // As wide as the code (and the header), up to the room there is.
    let galley = ui.fonts_mut(|f| f.layout_job(layout(code, language, palette)));
    let room = (ui.available_width() - 2.0 * FRAME_PAD - 2.0).max(HEADER_MIN);
    let inner = galley.size().x.max(HEADER_MIN).min(room);
    egui::Frame::new()
        .fill(palette.surface_hover)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(FRAME_PAD as i8, 6))
        .show(ui, |ui| {
            ui.set_width(inner);
            let copy = ui
                .horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&label)
                            .font(theme::mono(12.0))
                            .color(palette.dim),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::icon_button_sized(
                            ui,
                            palette,
                            Icon::Copy,
                            "Copy code",
                            24.0,
                            14.0,
                            palette.dim,
                        )
                    })
                    .inner
                })
                .inner;
            widgets::on_click(&copy, actions, Action::CopyText(code.to_owned()));
            ui.add_space(2.0);
            // A solid scroll bar, there only when a line is cut off.
            ui.scope(|ui| {
                widgets::thin_scrollbars(ui, palette);
                egui::ScrollArea::horizontal()
                    .id_salt("code-scroll")
                    .max_width(inner)
                    .auto_shrink([true, true])
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(galley).selectable(true));
                    });
            });
            copy
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found<'a>(code: &'a str, lang: Option<&str>) -> Vec<(&'a str, Token)> {
        tokens(code, lang)
            .into_iter()
            .map(|(range, token)| (&code[range], token))
            .collect()
    }

    #[test]
    fn line() {
        assert_eq!(
            tokens("let x = 1; // c", Some("rust")),
            vec![
                (0..3, Token::Keyword),
                (8..9, Token::Number),
                (11..15, Token::Comment)
            ]
        );
    }

    #[test]
    fn python_hash_comment() {
        assert_eq!(
            found("def f(): # note\n  return None", Some("py")),
            vec![
                ("def", Token::Keyword),
                ("# note", Token::Comment),
                ("return", Token::Keyword),
                ("None", Token::Keyword),
            ]
        );
    }

    #[test]
    fn sql_case_insensitive() {
        assert_eq!(
            found("SELECT a FROM t -- x", Some("SQL")),
            vec![
                ("SELECT", Token::Keyword),
                ("FROM", Token::Keyword),
                ("-- x", Token::Comment)
            ]
        );
        assert_eq!(
            found("select 1", Some("sql"))[0],
            ("select", Token::Keyword)
        );
    }

    #[test]
    fn unknown_language() {
        for lang in [None, Some("brainfuck"), Some("")] {
            assert_eq!(
                found("if x == \"s\" { 42 } // c", lang),
                vec![
                    ("\"s\"", Token::String),
                    ("42", Token::Number),
                    ("// c", Token::Comment)
                ]
            );
        }
    }

    #[test]
    fn unterminated_string() {
        assert_eq!(found("\"abc", None), vec![("\"abc", Token::String)]);
        assert_eq!(
            found("x = 'a\ny = 2", Some("python")),
            vec![("'a", Token::String), ("2", Token::Number)]
        );
        assert_eq!(found("`open\nmore", Some("js")).len(), 1);
        assert_eq!(
            found("/* open", Some("c")),
            vec![("/* open", Token::Comment)]
        );
        assert_eq!(found("\"\"\"doc", Some("py")).len(), 1);
        for input in ["'", "'\\", "\"\\", "'a", "/*", "\u{e9}'", "1.", "0x"] {
            for lang in [None, Some("rust"), Some("python"), Some("js")] {
                let _ = tokens(input, lang);
            }
        }
    }

    #[test]
    fn rust_lifetimes_are_not_strings() {
        assert_eq!(
            found("fn a<'a>(c: char) { 'x'; '\\n'; }", Some("rust")),
            vec![
                ("fn", Token::Keyword),
                ("'x'", Token::String),
                ("'\\n'", Token::String)
            ]
        );
    }

    #[test]
    fn utf8() {
        let code = "naïve = \"héllo wörld\" # ünï 🚀\nx = 3.14";
        for (range, _) in tokens(code, Some("python")) {
            assert!(code.is_char_boundary(range.start) && code.is_char_boundary(range.end));
        }
        assert_eq!(
            found(code, Some("python")),
            vec![
                ("\"héllo wörld\"", Token::String),
                ("# ünï 🚀", Token::Comment),
                ("3.14", Token::Number)
            ]
        );
        // Identifiers with digits are not numbers.
        assert!(found("x1 = y2", Some("py")).is_empty());
    }

    #[test]
    fn json_literals() {
        assert_eq!(
            found("{\"a\": [1, true, null]}", Some("json")),
            vec![
                ("\"a\"", Token::String),
                ("1", Token::Number),
                ("true", Token::Keyword),
                ("null", Token::Keyword)
            ]
        );
    }

    #[test]
    fn language_labels() {
        for (given, shown) in [
            (Some("rust"), "Rust"),
            (Some("js"), "JavaScript"),
            (Some("TS"), "TypeScript"),
            (Some("py"), "Python"),
            (Some("python"), "Python"),
            (Some("cs"), "C#"),
            (Some("cpp"), "C++"),
            (Some("sql"), "SQL"),
            (Some("json"), "JSON"),
            (Some("bash"), "Bash"),
            (Some("sh"), "Bash"),
            (Some("powershell"), "PowerShell"),
            (Some("ps1"), "PowerShell"),
            (Some("go"), "Go"),
            (Some("java"), "Java"),
            (Some("Brainfuck"), "Brainfuck"),
            (Some("  "), "Code"),
            (Some(""), "Code"),
            (None, "Code"),
        ] {
            assert_eq!(display_name(given), shown, "{given:?}");
        }
    }

    #[test]
    fn copy_pushes_action() {
        let ctx = egui::Context::default();
        crate::ui::install(&ctx, true);
        let palette = Palette::dark();
        let mut actions = Vec::new();
        let run = |events: Vec<egui::Event>, actions: &mut Vec<Action>| {
            let mut copy_rect = egui::Rect::NOTHING;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                copy_rect = copy_button(ui, &palette, "fn a() {}", Some("rust"), actions).rect;
            });
            output.textures_delta.clear();
            copy_rect
        };
        let at = run(vec![], &mut actions).center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(vec![egui::Event::PointerMoved(at)], &mut actions);
        run(vec![button(true)], &mut actions);
        assert!(actions.is_empty());
        run(vec![button(false)], &mut actions);
        assert!(matches!(&actions[..], [Action::CopyText(text)] if text == "fn a() {}"));
    }
}
