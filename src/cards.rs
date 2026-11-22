//! Adaptive Cards, read for display only. Chaat draws what a card says and
//! offers its links; it never submits a card or fetches its images.

use serde_json::Value;

/// A parsed card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub body: Vec<Element>,
    pub actions: Vec<CardAction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Element {
    Text {
        text: String,
        size: TextSize,
        bold: bool,
        subtle: bool,
    },
    Facts(Vec<(String, String)>),
    Container(Vec<Element>),
    Columns(Vec<Vec<Element>>),
    Image {
        alt: Option<String>,
        url: Option<String>,
    },
    Actions(Vec<CardAction>),
    /// An element Chaat does not draw, by its type name.
    Unsupported(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Medium,
    Large,
    ExtraLarge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardAction {
    OpenUrl { title: String, url: String },
    Unsupported { title: String },
}

const MAX_INPUT: usize = 256 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_ELEMENTS: usize = 200;

/// Reads a card's JSON. `None` when it is not a card Chaat can read.
pub fn parse(json: &str) -> Option<Card> {
    if json.len() > MAX_INPUT {
        return None;
    }
    let value: Value = serde_json::from_str(json).ok()?;
    let root = value.as_object()?;
    if root.get("type").and_then(Value::as_str) != Some("AdaptiveCard") {
        return None;
    }
    let mut reader = Reader { count: 0 };
    let body = reader.elements(root.get("body"), 1);
    let actions = actions(root.get("actions"));
    Some(Card { body, actions })
}

struct Reader {
    count: usize,
}

impl Reader {
    fn elements(&mut self, value: Option<&Value>, depth: usize) -> Vec<Element> {
        let Some(list) = value.and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for item in list {
            if self.count >= MAX_ELEMENTS {
                break;
            }
            self.count += 1;
            out.push(self.element(item, depth));
        }
        out
    }

    fn element(&mut self, value: &Value, depth: usize) -> Element {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if depth > MAX_DEPTH {
            return Element::Unsupported("depth".into());
        }
        match kind {
            "TextBlock" => Element::Text {
                text: str_of(value, "text"),
                size: size_of(value),
                bold: value
                    .get("weight")
                    .and_then(Value::as_str)
                    .is_some_and(|w| w.eq_ignore_ascii_case("bolder")),
                subtle: value
                    .get("isSubtle")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            "RichTextBlock" => {
                let text = value
                    .get("inlines")
                    .and_then(Value::as_array)
                    .map(|runs| {
                        runs.iter()
                            .map(|run| match run {
                                Value::String(s) => s.clone(),
                                _ => str_of(run, "text"),
                            })
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                Element::Text {
                    text,
                    size: TextSize::Default,
                    bold: false,
                    subtle: false,
                }
            }
            "FactSet" => Element::Facts(
                value
                    .get("facts")
                    .and_then(Value::as_array)
                    .map(|facts| {
                        facts
                            .iter()
                            .map(|f| (str_of(f, "title"), str_of(f, "value")))
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "Container" => Element::Container(self.elements(value.get("items"), depth + 1)),
            "ColumnSet" => {
                let mut columns = Vec::new();
                for column in value
                    .get("columns")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if self.count >= MAX_ELEMENTS {
                        break;
                    }
                    if column.get("type").and_then(Value::as_str) != Some("Column") {
                        continue;
                    }
                    self.count += 1;
                    columns.push(self.elements(column.get("items"), depth + 1));
                }
                Element::Columns(columns)
            }
            "Image" => Element::Image {
                alt: opt_str(value, "altText"),
                url: opt_str(value, "url"),
            },
            "ActionSet" => Element::Actions(actions(value.get("actions"))),
            other => Element::Unsupported(other.to_string()),
        }
    }
}

fn str_of(value: &Value, key: &str) -> String {
    opt_str(value, key).unwrap_or_default()
}

fn opt_str(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn size_of(value: &Value) -> TextSize {
    match value
        .get("size")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("small") => TextSize::Small,
        Some("medium") => TextSize::Medium,
        Some("large") => TextSize::Large,
        Some("extralarge") => TextSize::ExtraLarge,
        _ => TextSize::Default,
    }
}

fn actions(value: Option<&Value>) -> Vec<CardAction> {
    value
        .and_then(Value::as_array)
        .map(|list| list.iter().take(MAX_ELEMENTS).map(action).collect())
        .unwrap_or_default()
}

fn action(value: &Value) -> CardAction {
    let title = str_of(value, "title");
    if value.get("type").and_then(Value::as_str) == Some("Action.OpenUrl")
        && let Some(url) = value.get("url").and_then(Value::as_str)
    {
        let url = url.trim();
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("https://") || lower.starts_with("http://") {
            return CardAction::OpenUrl {
                title,
                url: url.to_string(),
            };
        }
    }
    CardAction::Unsupported { title }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(body: &str) -> String {
        format!(r#"{{"type":"AdaptiveCard","body":{body}}}"#)
    }

    #[test]
    fn text_example() {
        let c = parse(r#"{"type":"AdaptiveCard","body":[{"type":"TextBlock","text":"Hi","weight":"Bolder","size":"Large"}]}"#).unwrap();
        assert_eq!(
            c,
            Card {
                body: vec![Element::Text {
                    text: "Hi".into(),
                    size: TextSize::Large,
                    bold: true,
                    subtle: false
                }],
                actions: vec![]
            }
        );
    }

    #[test]
    fn facts_and_openurl_example() {
        let c = parse(r#"{"type":"AdaptiveCard","body":[{"type":"FactSet","facts":[{"title":"Owner","value":"Ada"}]}],"actions":[{"type":"Action.OpenUrl","title":"Open","url":"https://x.test"}]}"#).unwrap();
        assert_eq!(
            c.body,
            vec![Element::Facts(vec![("Owner".into(), "Ada".into())])]
        );
        assert_eq!(
            c.actions,
            vec![CardAction::OpenUrl {
                title: "Open".into(),
                url: "https://x.test".into()
            }]
        );
    }

    #[test]
    fn non_card_is_none() {
        assert_eq!(parse("[1]"), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(r#"{"type":"Other"}"#), None);
        assert_eq!(parse(r#"{"body":[]}"#), None);
    }

    #[test]
    fn depth_capped_at_8() {
        let mut body = String::from(r#"{"type":"TextBlock","text":"x"}"#);
        for _ in 0..9 {
            body = format!(r#"{{"type":"Container","items":[{body}]}}"#);
        }
        let c = parse(&card(&format!("[{body}]"))).unwrap();
        let mut cur = &c.body[0];
        for _ in 0..8 {
            let Element::Container(items) = cur else {
                panic!("expected container")
            };
            cur = &items[0];
        }
        assert_eq!(cur, &Element::Unsupported("depth".into()));
    }

    #[test]
    fn input_is_unsupported() {
        let c = parse(&card(r#"[{"type":"Input.Text","id":"a"}]"#)).unwrap();
        assert_eq!(c.body, vec![Element::Unsupported("Input.Text".into())]);
    }

    #[test]
    fn submit_is_unsupported_action() {
        let c = parse(
            r#"{"type":"AdaptiveCard","actions":[{"type":"Action.Submit","title":"Go"},{"type":"Action.Execute","title":"Run"}]}"#,
        )
        .unwrap();
        assert_eq!(
            c.actions,
            vec![
                CardAction::Unsupported { title: "Go".into() },
                CardAction::Unsupported {
                    title: "Run".into()
                }
            ]
        );
    }

    #[test]
    fn openurl_non_http_is_unsupported() {
        let c = parse(
            r#"{"type":"AdaptiveCard","actions":[{"type":"Action.OpenUrl","title":"X","url":"javascript:alert(1)"},{"type":"Action.OpenUrl","title":"F","url":"file:///c:/a"}]}"#,
        )
        .unwrap();
        assert!(
            c.actions
                .iter()
                .all(|a| matches!(a, CardAction::Unsupported { .. }))
        );
    }

    #[test]
    fn richtextblock_joins_runs() {
        let c = parse(&card(
            r#"[{"type":"RichTextBlock","inlines":[{"type":"TextRun","text":"Hello "},{"type":"TextRun","text":"world"}]}]"#,
        ))
        .unwrap();
        let Element::Text { text, .. } = &c.body[0] else {
            panic!()
        };
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn columnset_columns() {
        let c = parse(&card(
            r#"[{"type":"ColumnSet","columns":[{"type":"Column","items":[{"type":"TextBlock","text":"a"}]},{"type":"Column","items":[{"type":"TextBlock","text":"b"},{"type":"TextBlock","text":"c"}]}]}]"#,
        ))
        .unwrap();
        let Element::Columns(cols) = &c.body[0] else {
            panic!()
        };
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].len(), 1);
        assert_eq!(cols[1].len(), 2);
    }

    #[test]
    fn image_keeps_alt_and_url() {
        let c = parse(&card(
            r#"[{"type":"Image","altText":"logo","url":"https://x.test/a.png"}]"#,
        ))
        .unwrap();
        assert_eq!(
            c.body,
            vec![Element::Image {
                alt: Some("logo".into()),
                url: Some("https://x.test/a.png".into())
            }]
        );
    }

    #[test]
    fn oversize_input_rejected() {
        let big = "a".repeat(MAX_INPUT + 1);
        let json = card(&format!(r#"[{{"type":"TextBlock","text":"{big}"}}]"#));
        assert_eq!(parse(&json), None);
    }

    #[test]
    fn elements_capped_at_200() {
        let items = vec![r#"{"type":"TextBlock","text":"x"}"#; 500].join(",");
        let c = parse(&card(&format!("[{items}]"))).unwrap();
        assert_eq!(c.body.len(), 200);
    }

    #[test]
    fn unknown_type_unsupported_and_actionset() {
        let c = parse(&card(
            r#"[{"type":"Media"},{"type":"ActionSet","actions":[{"type":"Action.OpenUrl","title":"o","url":"http://a.test"}]}]"#,
        ))
        .unwrap();
        assert_eq!(c.body[0], Element::Unsupported("Media".into()));
        assert!(matches!(&c.body[1], Element::Actions(a) if a.len() == 1));
    }
}
