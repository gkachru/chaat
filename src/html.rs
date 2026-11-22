//! Teams message bodies, both ways.
//!
//! Graph hands chat and channel messages over as an HTML fragment. Chaat has
//! no browser engine, so [`parse`] reads the small subset Teams writes
//! (paragraphs, emphasis, links, code, quotes, lists, mentions, emoji,
//! inline images) into [`RichText`], which the interface lays out with egui.
//! Anything else degrades to its text. [`compose`] goes the other way: the
//! composer's light markdown becomes the HTML Graph accepts.
//!
//! The parser is a tolerant tokenizer rather than a full HTML5 parser: the
//! input is machine-written by Teams, and a hand-written loop keeps a large
//! dependency out of the binary. It never panics on malformed input.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub mention: bool,
    pub link: Option<String>,
    /// Font size in points, snapped to the type scale (`font-size` on a span).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u8>,
    /// Raised or lowered text (`<sup>`, `<sub>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<Script>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Script {
    Sup,
    Sub,
}

/// The type scale of `AGENTS.md`; message sizes snap to it.
const TYPE_SCALE: [u8; 9] = [11, 12, 13, 14, 15, 18, 20, 24, 30];

/// The scale step nearest to `size` (px treated as points); ties go down.
fn snap_size(size: f32) -> Option<u8> {
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let mut best = TYPE_SCALE[0];
    let mut best_gap = f32::MAX;
    for step in TYPE_SCALE {
        let gap = (f32::from(step) - size).abs();
        if gap < best_gap {
            best = step;
            best_gap = gap;
        }
    }
    Some(best)
}

/// The `font-size` of an inline `style` attribute, snapped.
fn font_size(style: &str) -> Option<u8> {
    style.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("font-size") {
            return None;
        }
        let value = value.trim().to_ascii_lowercase();
        let number = value
            .strip_suffix("px")
            .or_else(|| value.strip_suffix("pt"))?;
        snap_size(number.trim().parse::<f32>().ok()?)
    })
}

impl Style {
    fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    #[serde(default, skip_serializing_if = "Style::is_plain")]
    pub style: Style,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading(Vec<Span>),
    Quote(Vec<Span>),
    /// A list item; `Some(n)` numbers it.
    Item {
        ordered: Option<u32>,
        spans: Vec<Span>,
    },
    /// Old cached code blocks; new parses give [`Block::Snippet`].
    Code(String),
    /// A code block with the language Teams wrote, if any.
    Snippet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        text: String,
    },
    /// Rows of cells; `header` marks the first row as a header.
    Table {
        header: bool,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Image {
        src: String,
        alt: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
    },
    Rule,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RichText {
    pub blocks: Vec<Block>,
}

impl RichText {
    pub fn plain(text: &str) -> Self {
        let blocks = text
            .split('\n')
            .map(|line| {
                Block::Paragraph(vec![Span {
                    text: line.to_owned(),
                    style: Style::default(),
                }])
            })
            .collect();
        Self { blocks }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|block| match block {
            Block::Paragraph(spans)
            | Block::Heading(spans)
            | Block::Quote(spans)
            | Block::Item { spans, .. } => spans.iter().all(|span| span.text.trim().is_empty()),
            Block::Code(text) | Block::Snippet { text, .. } => text.trim().is_empty(),
            Block::Table { rows, .. } => rows.iter().all(|row| {
                row.iter()
                    .all(|cell| cell.iter().all(|s| s.text.trim().is_empty()))
            }),
            Block::Image { .. } | Block::Rule => false,
        })
    }

    /// The text alone, one line per block: for previews, notifications,
    /// search, and copying.
    pub fn to_plain(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            if !out.is_empty() {
                out.push('\n');
            }
            match block {
                Block::Paragraph(spans) | Block::Heading(spans) => push_spans(&mut out, spans),
                Block::Quote(spans) => {
                    out.push_str("> ");
                    push_spans(&mut out, spans);
                }
                Block::Item { ordered, spans } => {
                    match ordered {
                        Some(n) => out.push_str(&format!("{n}. ")),
                        None => out.push_str("• "),
                    }
                    push_spans(&mut out, spans);
                }
                Block::Code(code) => out.push_str(code),
                Block::Snippet { text, .. } => out.push_str(text),
                Block::Table { rows, .. } => {
                    for (index, row) in rows.iter().enumerate() {
                        if index > 0 {
                            out.push('\n');
                        }
                        for (cell_index, cell) in row.iter().enumerate() {
                            if cell_index > 0 {
                                out.push_str(" | ");
                            }
                            push_spans(&mut out, cell);
                        }
                    }
                }
                Block::Image { alt, .. } => out.push_str(
                    alt.as_deref()
                        .filter(|a| !a.is_empty())
                        .unwrap_or("[image]"),
                ),
                Block::Rule => out.push('—'),
            }
        }
        out
    }

    /// One line for the chat list.
    pub fn summary(&self, limit: usize) -> String {
        let flat: String = self
            .to_plain()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        truncate(&flat, limit)
    }

    /// Inline images, for lazy loading.
    pub fn images(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().filter_map(|block| match block {
            Block::Image { src, .. } => Some(src.as_str()),
            _ => None,
        })
    }
}

fn push_spans(out: &mut String, spans: &[Span]) {
    for span in spans {
        out.push_str(&span.text);
    }
}

/// Cuts `text` to at most `limit` characters, with an ellipsis.
pub fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

// ---------------------------------------------------------------------------
// Parsing

#[derive(Debug)]
enum Token<'a> {
    Text(&'a str),
    Open {
        name: String,
        attrs: Vec<(String, String)>,
        closed: bool,
    },
    Close(String),
}

/// Splits HTML into text, opening and closing tags. Comments, doctypes and
/// processing instructions are dropped.
fn tokenize(input: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // A '<' that does not start a tag is text.
        let next = bytes.get(i + 1).copied();
        let starts_tag = matches!(next, Some(b'/' | b'!' | b'?'))
            || next.is_some_and(|c| c.is_ascii_alphabetic());
        if !starts_tag {
            i += 1;
            continue;
        }
        if text_start < i {
            tokens.push(Token::Text(&input[text_start..i]));
        }
        if input[i..].starts_with("<!--") {
            let end = input[i + 4..]
                .find("-->")
                .map_or(input.len(), |at| i + 4 + at + 3);
            i = end;
            text_start = i;
            continue;
        }
        let Some(end) = find_tag_end(bytes, i + 1) else {
            // An unterminated tag: keep the rest as text.
            tokens.push(Token::Text(&input[i..]));
            text_start = input.len();
            break;
        };
        let inner = &input[i + 1..end];
        i = end + 1;
        text_start = i;
        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
        }
        if let Some(name) = inner.strip_prefix('/') {
            tokens.push(Token::Close(tag_name(name)));
            continue;
        }
        let closed = inner.trim_end().ends_with('/');
        let inner = inner.trim_end().trim_end_matches('/');
        let name = tag_name(inner);
        let attrs = parse_attrs(
            &inner[inner
                .find(|c: char| c.is_whitespace())
                .unwrap_or(inner.len())..],
        );
        tokens.push(Token::Open {
            name,
            attrs,
            closed,
        });
    }
    if text_start < input.len() {
        tokens.push(Token::Text(&input[text_start..]));
    }
    tokens
}

/// The `>` that ends a tag starting at `from`, skipping quoted attribute
/// values.
fn find_tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (offset, &byte) in bytes[from..].iter().enumerate() {
        match quote {
            Some(q) if byte == q => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(from + offset),
            None => {}
        }
    }
    None
}

fn tag_name(inner: &str) -> String {
    inner
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn parse_attrs(mut rest: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let name_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let value = if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            match after.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let body = &after[1..];
                    let close = body.find(q).unwrap_or(body.len());
                    rest = body.get(close + 1..).unwrap_or("");
                    decode_entities(&body[..close])
                }
                _ => {
                    let end = after.find(char::is_whitespace).unwrap_or(after.len());
                    rest = &after[end..];
                    decode_entities(&after[..end])
                }
            }
        } else {
            String::new()
        };
        if name.is_empty() {
            // Stray characters; skip one to make progress.
            let skip = rest.chars().next().map_or(0, char::len_utf8);
            rest = &rest[skip..];
            continue;
        }
        attrs.push((name, value));
    }
    attrs
}

/// The `language-xxx` class on a `<codeblock>`, `<pre>` or `<code>`.
fn language_of(attrs: &[(String, String)]) -> Option<String> {
    attr(attrs, "class")?
        .split_whitespace()
        .find_map(|class| class.strip_prefix("language-"))
        .filter(|language| !language.is_empty())
        .map(str::to_ascii_lowercase)
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Decodes character references: the named ones Teams and Outlook write,
/// and every numeric one.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(semi) = rest[1..].find(';').map(|i| i + 1).filter(|&i| i <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = if let Some(number) = entity.strip_prefix('#') {
            let value = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => number.parse::<u32>().ok(),
            };
            value.and_then(char::from_u32).map(String::from)
        } else {
            named_entity(entity).map(String::from)
        };
        match decoded {
            Some(value) => {
                out.push_str(&value);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "bull" => "•",
        "middot" => "·",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "euro" => "€",
        "pound" => "£",
        "times" => "×",
        "rarr" => "→",
        "larr" => "←",
        "zwj" => "\u{200d}",
        "zwnj" => "\u{200c}",
        _ => return None,
    })
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    spans: Vec<Span>,
    styles: Vec<(String, Style)>,
    quote_depth: usize,
    heading: bool,
    list: Vec<Option<u32>>,
    in_item: Option<Option<u32>>,
    pre: Option<String>,
    /// The language class of the open code block.
    pre_language: Option<String>,
    table: Option<TableBuilder>,
    /// Table nesting; inner tables flatten to text.
    table_depth: usize,
    /// Inside an `<emoji>` or Skype emoji span: its alt text has been used.
    skip_text: usize,
    /// Inside a `<style>` or `<script>` element.
    hidden: usize,
}

#[derive(Default)]
struct TableBuilder {
    rows: Vec<Vec<Vec<Span>>>,
    row: Vec<Vec<Span>>,
    row_has_th: bool,
    header: bool,
    in_cell: bool,
}

impl TableBuilder {
    fn end_row(&mut self) {
        if self.row.is_empty() {
            return;
        }
        if self.rows.is_empty() && self.row_has_th {
            self.header = true;
        }
        self.rows.push(std::mem::take(&mut self.row));
        self.row_has_th = false;
    }
}

/// Trims a run of spans the way a finished block is.
fn trim_spans(spans: &mut Vec<Span>) {
    if let Some(last) = spans.last_mut() {
        let trimmed = last.text.trim_end().len();
        last.text.truncate(trimmed);
    }
    if let Some(first) = spans.first_mut() {
        first.text = first.text.trim_start().to_owned();
    }
    spans.retain(|span| !span.text.is_empty());
}

impl Builder {
    /// Ends the open table cell, keeping its spans.
    fn end_cell(&mut self) {
        let mut spans = std::mem::take(&mut self.spans);
        if let Some(table) = &mut self.table {
            if table.in_cell {
                trim_spans(&mut spans);
                table.row.push(spans);
            }
            table.in_cell = false;
        }
    }

    fn style(&self) -> Style {
        self.styles
            .last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default()
    }

    fn push_style(&mut self, tag: &str, change: impl FnOnce(&mut Style)) {
        let mut style = self.style();
        change(&mut style);
        self.styles.push((tag.to_owned(), style));
    }

    fn pop_style(&mut self, tag: &str) {
        if let Some(index) = self.styles.iter().rposition(|(name, _)| name == tag) {
            self.styles.truncate(index);
        }
    }

    fn text(&mut self, raw: &str) {
        if self.hidden > 0 || self.skip_text > 0 {
            return;
        }
        let decoded = decode_entities(raw);
        if let Some(pre) = &mut self.pre {
            pre.push_str(&decoded);
            return;
        }
        // HTML whitespace collapses; a non-breaking space stays.
        let mut collapsed = String::with_capacity(decoded.len());
        let mut last_space = self
            .spans
            .last()
            .is_none_or(|span| span.text.ends_with(' '));
        for c in decoded.chars() {
            if c.is_whitespace() && c != '\u{a0}' {
                if !last_space {
                    collapsed.push(' ');
                    last_space = true;
                }
            } else {
                collapsed.push(if c == '\u{a0}' { ' ' } else { c });
                last_space = false;
            }
        }
        if collapsed.is_empty() {
            return;
        }
        self.push_text(collapsed, self.style());
    }

    fn push_text(&mut self, text: String, style: Style) {
        if let Some(last) = self.spans.last_mut()
            && last.style == style
        {
            last.text.push_str(&text);
            return;
        }
        self.spans.push(Span { text, style });
    }

    fn line_break(&mut self) {
        if let Some(pre) = &mut self.pre {
            pre.push('\n');
            return;
        }
        // A <br> ends the line but keeps the block's kind for the next one.
        self.flush();
    }

    /// Ends the current block of inline text, if it has any.
    fn flush(&mut self) {
        if self.table.is_some() {
            // Inside a table a block break is only a space between words.
            if self.spans.last().is_some_and(|s| !s.text.ends_with(' ')) {
                let style = self.style();
                self.push_text(" ".into(), style);
            }
            return;
        }
        let mut spans = std::mem::take(&mut self.spans);
        trim_spans(&mut spans);
        if spans.is_empty() {
            return;
        }
        let block = if let Some(ordered) = self.in_item {
            Block::Item { ordered, spans }
        } else if self.quote_depth > 0 {
            Block::Quote(spans)
        } else if self.heading {
            Block::Heading(spans)
        } else {
            Block::Paragraph(spans)
        };
        self.blocks.push(block);
    }

    fn open(&mut self, name: &str, attrs: &[(String, String)], closed: bool) {
        if self.hidden > 0 && !closed {
            if matches!(name, "style" | "script") {
                self.hidden += 1;
            }
            return;
        }
        match name {
            "style" | "script" | "title" | "head" if !closed => self.hidden += 1,
            "b" | "strong" => self.push_style(name, |s| s.bold = true),
            "i" | "em" => self.push_style(name, |s| s.italic = true),
            "u" | "ins" => self.push_style(name, |s| s.underline = true),
            "s" | "strike" | "del" => self.push_style(name, |s| s.strike = true),
            "code" if self.pre.is_none() => self.push_style(name, |s| s.code = true),
            "code" => {
                if self.pre_language.is_none() {
                    self.pre_language = language_of(attrs);
                }
            }
            "sup" => self.push_style(name, |s| s.script = Some(Script::Sup)),
            "sub" => self.push_style(name, |s| s.script = Some(Script::Sub)),
            "a" => {
                let href = attr(attrs, "href").map(str::to_owned);
                self.push_style(name, |s| s.link = href);
            }
            "at" => self.push_style(name, |s| s.mention = true),
            "span" => {
                let itemtype = attr(attrs, "itemtype").unwrap_or_default();
                if itemtype.ends_with("/Mention") {
                    self.push_style(name, |s| s.mention = true);
                } else if itemtype.ends_with("/Emoji") {
                    // The <img> inside carries the emoji in its alt.
                    self.push_style(name, |_| {});
                } else {
                    let size = attr(attrs, "style").and_then(font_size);
                    self.push_style(name, |s| {
                        if size.is_some() {
                            s.size = size;
                        }
                    });
                }
            }
            "emoji" => {
                if let Some(alt) = attr(attrs, "alt").filter(|a| !a.is_empty()) {
                    let style = self.style();
                    self.push_text(alt.to_owned(), style);
                }
                if !closed {
                    self.skip_text += 1;
                }
            }
            "br" => self.line_break(),
            "table" => {
                self.flush();
                self.table_depth += 1;
                if self.table_depth == 1 {
                    self.table = Some(TableBuilder::default());
                }
            }
            "tr" => {
                if self.table_depth == 1 {
                    self.end_cell();
                    if let Some(table) = &mut self.table {
                        table.end_row();
                    }
                    self.spans.clear();
                } else {
                    self.flush();
                }
            }
            "td" | "th" if self.table_depth == 1 => {
                self.end_cell();
                // Text between cells belongs to no cell.
                self.spans.clear();
                if let Some(table) = &mut self.table {
                    table.in_cell = true;
                    table.row_has_th |= name == "th";
                }
            }
            "p" | "div" | "section" | "article" | "header" | "footer" => {
                self.flush();
            }
            "td" | "th" => {
                if !self.spans.is_empty() {
                    let style = self.style();
                    self.push_text(" · ".into(), style);
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = true;
            }
            "blockquote" => {
                self.flush();
                self.quote_depth += 1;
            }
            "pre" | "codeblock" => {
                self.flush();
                self.pre = Some(String::new());
                self.pre_language = language_of(attrs);
            }
            "ul" => {
                self.flush();
                self.list.push(None);
            }
            "ol" => {
                self.flush();
                let start = attr(attrs, "start")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(1);
                self.list.push(Some(start));
            }
            "li" => {
                self.flush();
                let ordered = match self.list.last_mut() {
                    Some(Some(n)) => {
                        let current = *n;
                        *n += 1;
                        Some(current)
                    }
                    _ => None,
                };
                self.in_item = Some(ordered);
            }
            "hr" => {
                self.flush();
                self.blocks.push(Block::Rule);
            }
            "img" => self.image(attrs),
            _ => {}
        }
    }

    fn image(&mut self, attrs: &[(String, String)]) {
        let alt = attr(attrs, "alt").map(str::to_owned);
        let itemtype = attr(attrs, "itemtype").unwrap_or_default();
        let src = attr(attrs, "src").unwrap_or_default();
        // Teams draws its emoji as small images whose alt is the emoji.
        let is_emoji = itemtype.ends_with("/Emoji")
            || attr(attrs, "class").is_some_and(|c| c.contains("emoji"))
            || src.contains("/emoticons/");
        if is_emoji {
            if let Some(alt) = alt.filter(|a| !a.is_empty()) {
                let style = self.style();
                self.push_text(alt, style);
            }
            return;
        }
        if src.is_empty() {
            return;
        }
        self.flush();
        let size =
            |name| attr(attrs, name).and_then(|v| v.trim_end_matches("px").parse::<f32>().ok());
        self.blocks.push(Block::Image {
            src: src.to_owned(),
            alt,
            width: size("width"),
            height: size("height"),
        });
    }

    fn finish_pre(&mut self) {
        let language = self.pre_language.take();
        if let Some(code) = self.pre.take() {
            let text = code.trim_matches('\n').to_owned();
            if !text.is_empty() {
                self.blocks.push(Block::Snippet { language, text });
            }
        }
    }

    fn finish_table(&mut self) {
        self.end_cell();
        self.table_depth = 0;
        if let Some(mut table) = self.table.take() {
            table.end_row();
            if !table.rows.is_empty() {
                self.blocks.push(Block::Table {
                    header: table.header,
                    rows: table.rows,
                });
            }
        }
        self.spans.clear();
    }

    fn close(&mut self, name: &str) {
        if self.hidden > 0 {
            if matches!(name, "style" | "script" | "title" | "head") {
                self.hidden -= 1;
            }
            return;
        }
        match name {
            "b" | "strong" | "i" | "em" | "u" | "ins" | "s" | "strike" | "del" | "a" | "at"
            | "span" | "sup" | "sub" => self.pop_style(name),
            "code" if self.pre.is_none() => self.pop_style(name),
            "emoji" => self.skip_text = self.skip_text.saturating_sub(1),
            "tr" if self.table_depth == 1 => {
                self.end_cell();
                if let Some(table) = &mut self.table {
                    table.end_row();
                }
            }
            "td" | "th" if self.table_depth == 1 => self.end_cell(),
            "table" => {
                if self.table_depth == 1 {
                    self.finish_table();
                } else if self.table_depth > 1 {
                    self.table_depth -= 1;
                    self.flush();
                } else {
                    self.flush();
                }
            }
            "p" | "div" | "tr" | "section" | "article" | "header" | "footer" => self.flush(),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.heading = false;
            }
            "blockquote" => {
                self.flush();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            "pre" | "codeblock" => {
                self.finish_pre();
            }
            "li" => {
                self.flush();
                self.in_item = None;
            }
            "ul" | "ol" => {
                self.flush();
                self.list.pop();
                self.in_item = None;
            }
            _ => {}
        }
    }
}

/// Reads a Teams HTML body.
pub fn parse(html: &str) -> RichText {
    let mut builder = Builder::default();
    for token in tokenize(html) {
        match token {
            Token::Text(text) => builder.text(text),
            Token::Open {
                name,
                attrs,
                closed,
            } => builder.open(&name, &attrs, closed),
            Token::Close(name) => builder.close(&name),
        }
    }
    if builder.pre.is_some() {
        builder.finish_pre();
    }
    if builder.table.is_some() {
        builder.finish_table();
    }
    builder.flush();
    RichText {
        blocks: builder.blocks,
    }
}

// ---------------------------------------------------------------------------
// Composing

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Turns the composer's text into the HTML body Graph sends.
///
/// Supported: `**bold**`, `*italic*` / `_italic_`, `~~strike~~`,
/// `` `code` ``, fenced code blocks, `> quotes`, `- ` and `1. ` lists, and
/// bare `http(s)://` links. Everything else is escaped text; line breaks are
/// kept.
pub fn compose(text: &str) -> String {
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    let mut list: Option<&'static str> = None;
    let mut paragraph: Vec<String> = Vec::new();

    let flush_paragraph = |out: &mut String, paragraph: &mut Vec<String>| {
        if !paragraph.is_empty() {
            out.push_str("<p>");
            out.push_str(&paragraph.join("<br>"));
            out.push_str("</p>");
            paragraph.clear();
        }
    };
    let close_list = |out: &mut String, list: &mut Option<&'static str>| {
        if let Some(tag) = list.take() {
            out.push_str(&format!("</{tag}>"));
        }
    };

    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("```") {
            flush_paragraph(&mut out, &mut paragraph);
            close_list(&mut out, &mut list);
            let mut code = Vec::new();
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
                code.push(escape(inner));
            }
            out.push_str("<pre><code>");
            out.push_str(&code.join("\n"));
            out.push_str("</code></pre>");
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(quote) = trimmed
            .strip_prefix("> ")
            .or_else(|| (trimmed == ">").then_some(""))
        {
            flush_paragraph(&mut out, &mut paragraph);
            close_list(&mut out, &mut list);
            out.push_str("<blockquote>");
            out.push_str(&inline(quote));
            out.push_str("</blockquote>");
            continue;
        }
        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "));
        let numbered = trimmed
            .split_once(". ")
            .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .map(|(_, rest)| rest);
        if let Some((tag, item)) = bullet.map(|b| ("ul", b)).or(numbered.map(|n| ("ol", n))) {
            flush_paragraph(&mut out, &mut paragraph);
            if list != Some(tag) {
                close_list(&mut out, &mut list);
                out.push_str(&format!("<{tag}>"));
                list = Some(tag);
            }
            out.push_str("<li>");
            out.push_str(&inline(item));
            out.push_str("</li>");
            continue;
        }
        close_list(&mut out, &mut list);
        paragraph.push(inline(line));
    }
    flush_paragraph(&mut out, &mut paragraph);
    close_list(&mut out, &mut list);
    out
}

/// Like [`compose`], with `@Name` turned into Teams mentions.
///
/// `mentions` are the people picked in the composer, in order. The first
/// `@Name` in the text for each becomes `<at id="n">Name</at>`, numbered in
/// the order they appear; people whose `@Name` was deleted are dropped.
/// Returns the HTML and the mentions it uses, indexed by their `id`.
pub fn compose_with_mentions(
    text: &str,
    mentions: &[crate::model::MentionRef],
) -> (String, Vec<crate::model::MentionRef>) {
    // Mark each mention with private-use characters that markdown and
    // escaping leave alone, then swap the marks for <at> tags.
    let mut marked = text.to_owned();
    let mut found: Vec<(usize, crate::model::MentionRef)> = Vec::new();
    for mention in mentions {
        let needle = format!("@{}", mention.name);
        if found.iter().any(|(_, m)| m.user_id == mention.user_id) {
            continue;
        }
        let Some(at) = marked.find(&needle) else {
            continue;
        };
        found.push((at, mention.clone()));
        let token = format!("\u{E000}{}\u{E001}", found.len() - 1);
        marked.replace_range(at..at + needle.len(), &token);
    }
    // Number mentions by where they appear in the text, as Teams does.
    let mut order: Vec<usize> = (0..found.len()).collect();
    order.sort_by_key(|&index| {
        marked
            .find(&format!("\u{E000}{index}\u{E001}"))
            .unwrap_or(usize::MAX)
    });
    let mut html = compose(&marked);
    let mut used = Vec::new();
    for (id, &index) in order.iter().enumerate() {
        let mention = &found[index].1;
        html = html.replace(
            &format!("\u{E000}{index}\u{E001}"),
            &format!("<at id=\"{id}\">{}</at>", escape(&mention.name)),
        );
        used.push(mention.clone());
    }
    (html, used)
}

/// Inline markup for one line: code spans first (their content is literal),
/// then emphasis and links in the rest.
fn inline(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let Some(len) = rest[start + 1..].find('`') else {
            break;
        };
        out.push_str(&emphasis(&rest[..start]));
        out.push_str("<code>");
        out.push_str(&escape(&rest[start + 1..start + 1 + len]));
        out.push_str("</code>");
        rest = &rest[start + 1 + len + 1..];
    }
    out.push_str(&emphasis(rest));
    out
}

fn emphasis(text: &str) -> String {
    let linked = autolink(text);
    let mut out = linked;
    for (marker, tag) in [
        ("**", "strong"),
        ("++", "u"),
        ("~~", "s"),
        ("*", "em"),
        ("_", "em"),
    ] {
        out = wrap_pairs(&out, marker, tag);
    }
    out
}

/// Escapes text and turns bare links into anchors.
fn autolink(text: &str) -> String {
    let mut out = String::new();
    for (index, word) in text.split(' ').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let trimmed = word.trim_end_matches(['.', ',', ')', '!', '?', ';', ':']);
        if (trimmed.starts_with("https://") || trimmed.starts_with("http://")) && trimmed.len() > 8
        {
            let tail = &word[trimmed.len()..];
            let href = escape(trimmed);
            out.push_str(&format!("<a href=\"{href}\">{href}</a>"));
            out.push_str(&escape(tail));
        } else {
            out.push_str(&escape(word));
        }
    }
    out
}

/// Wraps text between pairs of `marker` in `tag`, leaving anchors alone.
/// A marker must touch a word on its inner side (`*a*`, not `a * b`).
fn wrap_pairs(text: &str, marker: &str, tag: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = find_marker(rest, marker, true) {
        let after = &rest[open + marker.len()..];
        let Some(close) = find_marker(after, marker, false) else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&format!("<{tag}>{}</{tag}>", &after[..close]));
        rest = &after[close + marker.len()..];
    }
    out.push_str(rest);
    out
}

fn find_marker(text: &str, marker: &str, opening: bool) -> Option<usize> {
    let mut from = 0;
    let mut in_tag = false;
    let bytes = text.as_bytes();
    while from < text.len() {
        let c = bytes[from];
        if c == b'<' {
            in_tag = true;
        } else if c == b'>' {
            in_tag = false;
        }
        if !in_tag && text[from..].starts_with(marker) {
            let before = text[..from].chars().next_back();
            let after = text[from + marker.len()..].chars().next();
            let ok = if opening {
                after.is_some_and(|c| !c.is_whitespace())
                    && before.is_none_or(|c| !c.is_alphanumeric())
            } else {
                before.is_some_and(|c| !c.is_whitespace())
                    && after.is_none_or(|c| !c.is_alphanumeric())
            };
            // Underscores inside words (snake_case) are not emphasis, and an
            // inner `*` of `**` is not a single marker.
            let doubled = marker.len() == 1
                && (text[from + 1..].starts_with(marker) || text[..from].ends_with(marker));
            if ok && !doubled {
                return Some(from);
            }
        }
        from += text[from..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(text: &str) -> Vec<Span> {
        vec![Span {
            text: text.into(),
            style: Style::default(),
        }]
    }

    #[test]
    fn plain_paragraphs_and_breaks() {
        let rich = parse("<p>Hello <b>world</b></p><p>Second<br>line</p>");
        assert_eq!(rich.to_plain(), "Hello world\nSecond\nline");
        let Block::Paragraph(first) = &rich.blocks[0] else {
            panic!("paragraph expected");
        };
        assert!(first[1].style.bold);
    }

    #[test]
    fn whitespace_collapses_but_code_keeps_it() {
        let rich = parse("<div>a   \n  b</div><pre>x  =  1\n  y</pre>");
        assert_eq!(rich.blocks[0], Block::Paragraph(spans("a b")));
        assert_eq!(
            rich.blocks[1],
            Block::Snippet {
                language: None,
                text: "x  =  1\n  y".into()
            }
        );
    }

    #[test]
    fn entities_decode() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &#128512; &#x1F600; &nbsp;"),
            "a & b <c> 😀 😀 \u{a0}"
        );
        assert_eq!(decode_entities("AT&T; & &unknown;"), "AT&T; & &unknown;");
    }

    #[test]
    fn mentions_links_and_emoji() {
        let rich = parse(
            r#"<p><at id="0">Ada Lovelace</at> see <a href="https://example.com/x?a=1&amp;b=2">this</a> <emoji id="smile" alt="😄" title="Smile"></emoji></p>"#,
        );
        let Block::Paragraph(spans) = &rich.blocks[0] else {
            panic!("paragraph expected");
        };
        assert!(spans[0].style.mention);
        assert_eq!(spans[0].text, "Ada Lovelace");
        let link = spans.iter().find(|s| s.style.link.is_some()).unwrap();
        assert_eq!(
            link.style.link.as_deref(),
            Some("https://example.com/x?a=1&b=2")
        );
        assert_eq!(rich.to_plain(), "Ada Lovelace see this 😄");
    }

    #[test]
    fn skype_emoji_images_become_text() {
        let rich = parse(
            r#"<p>Nice <span itemtype="http://schema.skype.com/Emoji" itemscope=""><img src="https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/like/default/20_f.png" alt="👍" itemtype="http://schema.skype.com/Emoji" style="width:20px"></span></p>"#,
        );
        assert_eq!(rich.to_plain(), "Nice 👍");
        assert_eq!(rich.images().count(), 0);
    }

    #[test]
    fn hosted_images_are_blocks() {
        let rich = parse(
            r#"<p>Look:</p><p><img src="https://graph.microsoft.com/v1.0/chats/19:x/messages/1/hostedContents/aW1n/$value" width="250" height="140" alt="image"></p>"#,
        );
        assert_eq!(rich.images().count(), 1);
        assert!(matches!(
            rich.blocks[1],
            Block::Image {
                width: Some(250.0),
                ..
            }
        ));
    }

    #[test]
    fn lists_quotes_and_headings() {
        let rich = parse(
            "<h2>Plan</h2><ol><li>One</li><li>Two</li></ol><ul><li>Dot</li></ul><blockquote>Wise</blockquote><hr>",
        );
        assert_eq!(rich.to_plain(), "Plan\n1. One\n2. Two\n• Dot\n> Wise\n—");
        assert!(matches!(rich.blocks[0], Block::Heading(_)));
    }

    #[test]
    fn malformed_input_never_panics() {
        for input in [
            "<",
            "<<<>>>",
            "<p",
            "<a href='x",
            "</b></i>text",
            "<!-- open",
            "a < b > c",
            "&",
            "&#xZZ;",
            "<img src=>",
            "<p>ünïcödé <b>ok",
            "<at>",
            "<emoji alt='x'>",
        ] {
            let _ = parse(input).to_plain();
        }
        assert_eq!(parse("a < b > c").to_plain(), "a < b > c");
    }

    #[test]
    fn style_and_script_are_dropped() {
        let rich = parse("<style>p { color: red }</style><p>visible</p><script>alert(1)</script>");
        assert_eq!(rich.to_plain(), "visible");
    }

    #[test]
    fn compose_escapes_and_formats() {
        assert_eq!(compose("a < b & c"), "<p>a &lt; b &amp; c</p>");
        assert_eq!(
            compose("**bold** and *it* and `x<y`"),
            "<p><strong>bold</strong> and <em>it</em> and <code>x&lt;y</code></p>"
        );
        assert_eq!(compose("line1\nline2"), "<p>line1<br>line2</p>");
        assert_eq!(
            compose("snake_case_name stays"),
            "<p>snake_case_name stays</p>"
        );
        assert_eq!(compose("2 * 3 * 4"), "<p>2 * 3 * 4</p>");
        assert_eq!(compose("~~gone~~"), "<p><s>gone</s></p>");
    }

    #[test]
    fn compose_links_lists_quotes_code() {
        assert_eq!(
            compose("see https://example.com/a_b_c."),
            "<p>see <a href=\"https://example.com/a_b_c\">https://example.com/a_b_c</a>.</p>"
        );
        assert_eq!(
            compose("- a\n- b\ntext"),
            "<ul><li>a</li><li>b</li></ul><p>text</p>"
        );
        assert_eq!(compose("1. a\n2. b"), "<ol><li>a</li><li>b</li></ol>");
        assert_eq!(compose("> quoted"), "<blockquote>quoted</blockquote>");
        assert_eq!(
            compose("```\nfn x() {}\n<tag>\n```"),
            "<pre><code>fn x() {}\n&lt;tag&gt;</code></pre>"
        );
    }

    #[test]
    fn compose_round_trips_through_parse() {
        let text = "Hello **team**\n- first\n- second";
        assert_eq!(
            parse(&compose(text)).to_plain(),
            "Hello team\n• first\n• second"
        );
    }

    #[test]
    fn mentions_become_at_tags_in_text_order() {
        use crate::model::MentionRef;
        let ada = MentionRef {
            user_id: "u-ada".into(),
            name: "Ada Lovelace".into(),
        };
        let grace = MentionRef {
            user_id: "u-grace".into(),
            name: "Grace <Hopper>".into(),
        };
        let gone = MentionRef {
            user_id: "u-gone".into(),
            name: "Alan Turing".into(),
        };
        let (html, used) = compose_with_mentions(
            "@Grace <Hopper> and @Ada Lovelace: **ship** it",
            &[ada.clone(), grace.clone(), gone],
        );
        assert_eq!(
            html,
            "<p><at id=\"0\">Grace &lt;Hopper&gt;</at> and <at id=\"1\">Ada Lovelace</at>: <strong>ship</strong> it</p>"
        );
        assert_eq!(used, vec![grace, ada]);
        let (plain, none) = compose_with_mentions("no mentions", &[]);
        assert_eq!(plain, "<p>no mentions</p>");
        assert!(none.is_empty());
    }

    #[test]
    fn old_cache_json() {
        let block: Block = serde_json::from_str(r#"{"Code":"x"}"#).unwrap();
        assert_eq!(block, Block::Code("x".into()));
        let span: Span = serde_json::from_str(
            r#"{"text":"a","style":{"bold":true,"italic":false,"underline":false,"strike":false,"code":false,"mention":false,"link":null}}"#,
        )
        .unwrap();
        assert!(span.style.bold && span.style.size.is_none() && span.style.script.is_none());
        let rich = RichText {
            blocks: vec![Block::Snippet {
                language: Some("rust".into()),
                text: "x".into(),
            }],
        };
        let back: RichText = serde_json::from_str(&serde_json::to_string(&rich).unwrap()).unwrap();
        assert_eq!(back, rich);
    }

    #[test]
    fn snippet_languages() {
        let rich = parse("<codeblock class=\"language-rust\"><code>fn a()</code></codeblock>");
        assert_eq!(
            rich.blocks,
            vec![Block::Snippet {
                language: Some("rust".into()),
                text: "fn a()".into()
            }]
        );
        let rich = parse("<pre><code class=\"language-Python\">x = 1\n</code></pre><pre>y</pre>");
        assert_eq!(
            rich.blocks[0],
            Block::Snippet {
                language: Some("python".into()),
                text: "x = 1".into()
            }
        );
        assert_eq!(
            rich.blocks[1],
            Block::Snippet {
                language: None,
                text: "y".into()
            }
        );
        // Unclosed block at the end still becomes a snippet.
        assert_eq!(parse("<pre>open").blocks.len(), 1);
    }

    #[test]
    fn size_snaps() {
        for (px, want) in [(16, 15), (21, 20), (40, 30), (24, 24), (1, 11)] {
            let rich = parse(&format!(
                "<p><span style=\"font-size:{px}px\">big</span></p>"
            ));
            let Block::Paragraph(spans) = &rich.blocks[0] else {
                panic!("paragraph expected");
            };
            assert_eq!(spans[0].style.size, Some(want), "{px}px");
        }
        let rich = parse("<span style=\"color:red; font-size: 18pt\">x</span>y");
        let Block::Paragraph(spans) = &rich.blocks[0] else {
            panic!()
        };
        assert_eq!(spans[0].style.size, Some(18));
        assert_eq!(spans[1].style.size, None);
        assert_eq!(font_size("font-size:NaNpx"), None);
        assert_eq!(font_size("font-size:1.5em"), None);
    }

    #[test]
    fn sup_sub() {
        let rich = parse("<p>x<sup>2</sup> H<sub>2</sub>O</p>");
        let Block::Paragraph(spans) = &rich.blocks[0] else {
            panic!()
        };
        assert_eq!(spans[1].style.script, Some(Script::Sup));
        assert_eq!(spans[3].style.script, Some(Script::Sub));
        assert_eq!(spans[4].style.script, None);
        assert_eq!(rich.to_plain(), "x2 H2O");
    }

    #[test]
    fn table_rows() {
        let rich = parse(
            "<p>before</p><table><thead><tr><th>A</th><th>B</th></tr></thead><tbody><tr><td>1</td><td><b>2</b></td></tr><tr><td>3</td></tr></tbody></table><p>after</p>",
        );
        assert_eq!(rich.blocks.len(), 3);
        let Block::Table { header, rows } = &rich.blocks[1] else {
            panic!("table expected");
        };
        assert!(*header);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][1][0].text, "B");
        assert!(rows[1][1][0].style.bold);
        assert_eq!(rows[2].len(), 1);
        assert_eq!(rich.to_plain(), "before\nA | B\n1 | 2\n3\nafter");
        let plain = parse("<table><tr><td>x</td></tr></table>");
        assert!(matches!(
            &plain.blocks[0],
            Block::Table { header: false, .. }
        ));
    }

    #[test]
    fn nested_table() {
        let rich = parse(
            "<table><tr><td>out<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>tail</td><td>z</td></tr></table>after",
        );
        assert_eq!(rich.blocks.len(), 2);
        let Block::Table { rows, .. } = &rich.blocks[0] else {
            panic!("table expected");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 2);
        let first: String = rows[0][0].iter().map(|s| s.text.as_str()).collect();
        assert!(first.starts_with("out") && first.contains('a') && first.contains('c'));
        assert!(first.ends_with("tail"));
        assert_eq!(rows[0][1][0].text, "z");
        assert_eq!(rich.blocks[1].clone(), Block::Paragraph(spans("after")));
    }

    #[test]
    fn malformed_tables_and_blocks() {
        for input in [
            "<table>",
            "<table><tr>",
            "<table><tr><td>ü",
            "</table></td></tr>x",
            "<table><table></table>",
            "<td>a</td>",
            "<pre><code class='language-",
            "<codeblock class=\"language-\">é",
            "<span style=\"font-size:\">x</span>",
            "<span style=\"font-size:é\">x</span>",
        ] {
            let _ = parse(input).to_plain();
        }
        assert_eq!(parse("<table>").blocks.len(), 0);
        assert_eq!(parse("<table><tr><td>a").blocks.len(), 1);
    }

    #[test]
    fn new_style_fields_stay_out_of_stored_json() {
        let rich = parse("<p>plain <b>bold</b></p><pre>x</pre>");
        let json = serde_json::to_string(&rich).unwrap();
        assert!(
            !json.contains("size") && !json.contains("script") && !json.contains("language"),
            "{json}"
        );
        let sup = parse("<p>a<sup>2</sup></p>");
        let back: RichText = serde_json::from_str(&serde_json::to_string(&sup).unwrap()).unwrap();
        assert_eq!(back, sup);
    }

    #[test]
    fn sup_inside_code_and_deep_nesting() {
        let _ = parse("<pre>a<sup>2</sup>b</pre>");
        let _ = parse("<code>x<sup>é</sup></code>");
        let deep = "<table><tr><td>".repeat(50) + "x" + &"</td></tr></table>".repeat(50);
        assert_eq!(parse(&deep).blocks.len(), 1);
        let _ = parse(&"<sup>".repeat(200));
    }

    #[test]
    fn compose_underline() {
        assert_eq!(compose("++u++"), "<p><u>u</u></p>");
        assert_eq!(compose("i++ and j++"), "<p>i++ and j++</p>");
    }

    #[test]
    fn summary_flattens_and_truncates() {
        let rich = parse("<p>one</p><p>two   three</p>");
        assert_eq!(rich.summary(100), "one two three");
        assert_eq!(rich.summary(5), "one …");
    }
}
