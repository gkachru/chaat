//! The plugin protocol: newline-delimited JSON, each message with `"v": 1`
//! and a `type` (docs/calling.md#protocol). The same messages live as test
//! fixtures in `tests/fixtures/plugin-protocol/`, copied unchanged into the
//! plugin's repository, so the two sides cannot drift apart unnoticed.
//!
//! Everything here is plugin-neutral: identities are Entra object and tenant
//! ids and sign-in names. How a plugin learns them is its own business.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The protocol version Chaat speaks. Messages with another `v` are dropped.
pub const VERSION: u64 = 1;

/// The longest line Chaat reads from a plugin; longer lines are dropped.
pub const MAX_LINE: usize = 64 * 1024;

/// One of Chaat's accounts, as a plugin is told about it. Never a token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAccount {
    /// `oid.tid`.
    pub id: String,
    /// The sign-in name, possibly empty while Chaat doesn't know it yet.
    pub username: String,
}

/// A message from Chaat to a plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    /// Sent once at each start.
    Hello {
        host: String,
        capabilities: Vec<String>,
        accounts: Vec<PluginAccount>,
        /// Accounts signed out of Chaat while the plugin wasn't running: the
        /// plugin forgets their sign-ins before it starts.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        forget: Vec<String>,
    },
    /// Replaces the account list.
    Accounts {
        accounts: Vec<PluginAccount>,
    },
    /// Forget the plugin's sign-in for this account.
    Forget {
        account: String,
    },
    /// Offer a fresh sign-in for this account now.
    SignIn {
        account: String,
    },
    /// The current option values (secrets included), after `ready` and after
    /// each change. Keys are option ids.
    Options {
        values: BTreeMap<String, serde_json::Value>,
    },
    /// The busiest presence across accounts; only for plugins that need it.
    Presence {
        availability: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        activity: Option<String>,
    },
    Shutdown,
}

impl HostMessage {
    /// The message as one line, with its version, ending in a newline.
    pub fn to_line(&self) -> String {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        value["v"] = VERSION.into();
        let mut line = value.to_string();
        line.push('\n');
        line
    }
}

/// A message from a plugin to Chaat. Unknown types parse as `Other`, so a
/// newer plugin cannot break an older Chaat.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginMessage {
    /// The reply to `hello`: what the plugin offers, and how it describes
    /// itself (untrusted text; see `PluginInfo::from_ready`).
    Ready {
        #[serde(default, deserialize_with = "lenient_strings")]
        capabilities: Vec<String>,
        #[serde(default, deserialize_with = "lenient_text")]
        name: Option<String>,
        #[serde(default, deserialize_with = "lenient_text")]
        description: Option<String>,
        #[serde(default, deserialize_with = "lenient_text")]
        docs_url: Option<String>,
        /// The `hello.forget` accounts whose sign-ins the plugin cleared.
        #[serde(default, deserialize_with = "lenient_strings")]
        forgotten: Vec<String>,
        /// What the plugin asks for (`accounts`, `presence`); a non-array
        /// reads as none.
        #[serde(default, deserialize_with = "lenient_strings")]
        needs: Vec<String>,
        /// Raw option descriptions; see `clean_options`. A non-array reads
        /// as none.
        #[serde(default, deserialize_with = "lenient_array")]
        options: Vec<serde_json::Value>,
    },
    SignInRequired {
        /// How to sign in. Absent means `device_code` (older plugins).
        #[serde(default, deserialize_with = "lenient_text")]
        method: Option<String>,
        verification_uri: String,
        user_code: String,
        #[serde(default, deserialize_with = "lenient_text")]
        account: Option<String>,
        #[serde(default, deserialize_with = "lenient_text")]
        username: Option<String>,
    },
    SignedIn {
        #[serde(default, deserialize_with = "lenient_text")]
        account: Option<String>,
        #[serde(default, deserialize_with = "lenient_text")]
        username: Option<String>,
    },
    Listening {
        account: String,
        #[serde(default, deserialize_with = "lenient_text")]
        username: Option<String>,
    },
    IncomingCall {
        id: String,
        #[serde(default, deserialize_with = "lenient_text")]
        caller: Option<String>,
        /// The caller's Entra object id.
        #[serde(default, deserialize_with = "lenient_text")]
        caller_object_id: Option<String>,
        #[serde(default, deserialize_with = "lenient_bool")]
        video: bool,
        #[serde(default, deserialize_with = "lenient_bool")]
        group: bool,
        #[serde(default, deserialize_with = "lenient_text")]
        thread_id: Option<String>,
        #[serde(default, deserialize_with = "lenient_text")]
        account: Option<String>,
        #[serde(default, deserialize_with = "lenient_text")]
        username: Option<String>,
        /// How long the call rings, so the card can close with it.
        #[serde(default, deserialize_with = "lenient_seconds")]
        ring_timeout_secs: Option<u64>,
    },
    Error {
        #[serde(default, deserialize_with = "lenient_text")]
        message: Option<String>,
        #[serde(default, deserialize_with = "lenient_text")]
        account: Option<String>,
    },
    #[serde(other)]
    Other,
}

impl PluginMessage {
    /// Parses one line. `None` for anything that isn't a version-1 message.
    pub fn parse(line: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if value.get("v").and_then(serde_json::Value::as_u64) != Some(VERSION) {
            return None;
        }
        serde_json::from_value(value).ok()
    }
}

/// How a plugin describes itself, cleaned up for display: plain text only,
/// short, and a documentation link only when it is HTTPS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginInfo {
    pub name: Option<String>,
    pub description: Option<String>,
    pub docs_url: Option<String>,
}

impl PluginInfo {
    pub fn from_ready(
        name: Option<String>,
        description: Option<String>,
        docs_url: Option<String>,
    ) -> Self {
        Self {
            name: name.and_then(|name| plain(&name, 60)),
            description: description.and_then(|text| plain(&text, 300)),
            docs_url: docs_url
                .filter(|url| url.starts_with("https://") && url.len() <= 300)
                .filter(|url| !url.chars().any(|c| c.is_whitespace() || c.is_control()))
                .filter(|url| !has_userinfo(url)),
        }
    }
}

/// Whether an `https://` address has user info: an `@` in its authority,
/// before the first `/`, `?` or `#`.
fn has_userinfo(url: &str) -> bool {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority.contains('@')
}

/// An account id (`oid.tid`) from a plugin: letters, digits and dashes around
/// one dot, at most 100 characters. Anything else is `None`.
pub fn account_id(text: &str) -> Option<String> {
    let (oid, tid) = text.split_once('.')?;
    let part_ok = |part: &str| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    (text.len() <= 100 && part_ok(oid) && part_ok(tid)).then(|| text.to_owned())
}

/// The most options a plugin may describe.
const MAX_OPTIONS: usize = 16;
/// The most choices one `choice` option may offer.
const MAX_CHOICES: usize = 12;

/// What kind of control an option is drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    Toggle,
    Choice,
    Text,
    Secret,
}

/// One choice of a `choice` option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionChoice {
    pub value: String,
    pub label: String,
}

/// A plugin-described setting, cleaned for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionSpec {
    /// `[a-z0-9_]{1,32}`.
    pub id: String,
    pub kind: OptionKind,
    pub label: String,
    /// A bool for `toggle`, a string for `text`, one of the choice values for
    /// `choice`, never present for `secret`.
    pub default: Option<serde_json::Value>,
    pub choices: Vec<OptionChoice>,
}

/// Cleans untrusted `ready.options`: drops what is malformed (bad id, kind,
/// label or default), keeps the first of a duplicate id and the first 16.
pub fn clean_options(raw: Vec<serde_json::Value>) -> Vec<OptionSpec> {
    let mut seen = BTreeSet::new();
    let mut specs = Vec::new();
    for item in raw {
        let Some(spec) = clean_option(&item) else {
            continue;
        };
        if seen.insert(spec.id.clone()) {
            specs.push(spec);
            if specs.len() == MAX_OPTIONS {
                break;
            }
        }
    }
    specs
}

fn clean_option(item: &serde_json::Value) -> Option<OptionSpec> {
    let id = item.get("id")?.as_str()?;
    let id_ok = (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !id_ok {
        return None;
    }
    let kind = match item.get("kind")?.as_str()? {
        "toggle" => OptionKind::Toggle,
        "choice" => OptionKind::Choice,
        "text" => OptionKind::Text,
        "secret" => OptionKind::Secret,
        _ => return None,
    };
    let label = plain(item.get("label")?.as_str()?, 60)?;
    let default = item.get("default").filter(|v| !v.is_null());
    let mut choices = Vec::new();
    let default = match kind {
        OptionKind::Toggle => match default {
            None => None,
            Some(v) if v.is_boolean() => Some(v.clone()),
            Some(_) => return None,
        },
        OptionKind::Text => match default {
            None => None,
            Some(v) => {
                let text = v.as_str()?;
                if text.chars().count() > 200
                    || text.chars().any(|c| c.is_control() || is_invisible(c))
                {
                    return None;
                }
                Some(v.clone())
            }
        },
        OptionKind::Secret => {
            if default.is_some() {
                return None;
            }
            None
        }
        OptionKind::Choice => {
            let list = item.get("choices")?.as_array()?;
            if list.is_empty() || list.len() > MAX_CHOICES {
                return None;
            }
            for entry in list {
                let value = entry.get("value")?.as_str()?;
                let label = plain(entry.get("label")?.as_str()?, 60)?;
                if value.is_empty()
                    || value.chars().count() > 64
                    || value.chars().any(|c| c.is_control() || is_invisible(c))
                    || choices.iter().any(|c: &OptionChoice| c.value == value)
                {
                    return None;
                }
                choices.push(OptionChoice {
                    value: value.to_owned(),
                    label,
                });
            }
            match default {
                None => None,
                Some(v) => {
                    let text = v.as_str()?;
                    choices.iter().any(|c| c.value == text).then(|| v.clone())?;
                    Some(v.clone())
                }
            }
        }
    };
    Some(OptionSpec {
        id: id.to_owned(),
        kind,
        label,
        default,
        choices,
    })
}

/// Invisible formatting characters (direction overrides, zero-width marks)
/// that could make a plugin's text read differently from what it says.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{034F}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{3164}'
            | '\u{FFA0}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// The strings of a JSON array; anything else is empty.
fn lenient_strings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_json::Value::String(text) => Some(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// The items of a JSON array; anything else is empty.
fn lenient_array<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<serde_json::Value>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => items,
        _ => Vec::new(),
    })
}

/// A string field, or `None` for anything else.
fn lenient_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(text) => Some(text),
        _ => None,
    })
}

/// `true` only for a JSON `true`.
fn lenient_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(d)? == serde_json::Value::Bool(true))
}

/// Whole seconds from any non-negative number; `None` for anything else.
fn lenient_seconds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(|s| s.round().min(u64::MAX as f64) as u64)
        }),
        _ => None,
    })
}

/// Plugin text made safe to show: `plain`, then trusted no further.
pub fn plain_text(text: &str, max: usize) -> Option<String> {
    plain(text, max)
}

/// `text` without control characters, collapsed to single spaces, cut to
/// `max` characters (with an ellipsis); `None` when nothing is left.
fn plain(text: &str, max: usize) -> Option<String> {
    let visible: String = text.chars().filter(|c| !is_invisible(*c)).collect();
    let words: Vec<&str> = visible
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect();
    let text = words.join(" ");
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= max {
        return Some(text);
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    Some(format!("{}…", cut.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/plugin-protocol"
    );

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{FIXTURES}/{name}.json")).expect("fixture exists")
    }

    fn value(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    fn ada() -> PluginAccount {
        PluginAccount {
            id: "o1.t1".into(),
            username: "ada@contoso.com".into(),
        }
    }

    #[test]
    fn every_fixture_is_one_line_of_version_1_json() {
        for entry in std::fs::read_dir(FIXTURES).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let text = std::fs::read_to_string(&path).unwrap();
                assert_eq!(text.trim_end().lines().count(), 1, "{path:?}");
                assert_eq!(value(&text)["v"], 1, "{path:?}");
            }
        }
    }

    #[test]
    fn what_chaat_sends_matches_the_fixtures() {
        let sent = [
            (
                "hello",
                HostMessage::Hello {
                    host: "chaat".into(),
                    capabilities: vec!["call_alerts".into()],
                    accounts: vec![ada()],
                    forget: vec!["o9.t9".into()],
                },
            ),
            (
                "accounts",
                HostMessage::Accounts {
                    accounts: vec![
                        ada(),
                        PluginAccount {
                            id: "o2.t2".into(),
                            username: String::new(),
                        },
                    ],
                },
            ),
            (
                "forget",
                HostMessage::Forget {
                    account: "o9.t9".into(),
                },
            ),
            (
                "sign_in",
                HostMessage::SignIn {
                    account: "o1.t1".into(),
                },
            ),
            ("shutdown", HostMessage::Shutdown),
        ];
        for (name, message) in sent {
            let line = message.to_line();
            assert!(line.ends_with('\n') && line.trim_end().lines().count() == 1);
            assert_eq!(value(&line), value(&fixture(name)), "{name}");
        }
    }

    #[test]
    fn what_plugins_send_parses_from_the_fixtures() {
        assert_eq!(
            PluginMessage::parse(&fixture("ready")),
            Some(PluginMessage::Ready {
                capabilities: vec!["call_alerts".into()],
                name: Some("Ring alerts".into()),
                description: Some("Tells Chaat who is calling you in Teams.".into()),
                docs_url: Some("https://github.com/gkachru/call-alerts".into()),
                forgotten: vec!["o9.t9".into()],
                needs: vec![],
                options: vec![],
            })
        );
        assert_eq!(
            PluginMessage::parse(&fixture("sign_in_required")),
            Some(PluginMessage::SignInRequired {
                method: Some("device_code".into()),
                verification_uri: "https://microsoft.com/devicelogin".into(),
                user_code: "ABCD1234".into(),
                account: Some("o1.t1".into()),
                username: Some("ada@contoso.com".into()),
            })
        );
        assert_eq!(
            PluginMessage::parse(&fixture("signed_in")),
            Some(PluginMessage::SignedIn {
                account: Some("o1.t1".into()),
                username: Some("ada@contoso.com".into()),
            })
        );
        assert_eq!(
            PluginMessage::parse(&fixture("listening")),
            Some(PluginMessage::Listening {
                account: "o1.t1".into(),
                username: Some("ada@contoso.com".into()),
            })
        );
        assert_eq!(
            PluginMessage::parse(&fixture("incoming_call")),
            Some(PluginMessage::IncomingCall {
                id: "call-1".into(),
                caller: Some("Grace Hopper".into()),
                caller_object_id: Some("o2".into()),
                video: true,
                group: false,
                thread_id: Some("19:group@thread.v2".into()),
                account: Some("o1.t1".into()),
                username: Some("ada@contoso.com".into()),
                ring_timeout_secs: Some(40),
            })
        );
        assert_eq!(
            PluginMessage::parse(&fixture("error")),
            Some(PluginMessage::Error {
                message: Some("could not find the call service".into()),
                account: Some("o1.t1".into()),
            })
        );
    }

    #[test]
    fn other_versions_garbage_and_unknown_types_are_handled() {
        assert_eq!(PluginMessage::parse(r#"{"v":2,"type":"ready"}"#), None);
        assert_eq!(PluginMessage::parse(r#"{"type":"ready"}"#), None);
        assert_eq!(PluginMessage::parse("not json"), None);
        assert_eq!(
            PluginMessage::parse(r#"{"v":1,"type":"presence","who":"x"}"#),
            Some(PluginMessage::Other)
        );
    }

    #[test]
    fn a_mistyped_ring_field_does_not_drop_the_ring() {
        let ring = PluginMessage::parse(
            r#"{"v":1,"type":"incoming_call","id":"c","video":null,"group":"yes","ring_timeout_secs":40.4,"caller":7}"#,
        );
        assert!(matches!(
            ring,
            Some(PluginMessage::IncomingCall {
                video: false,
                group: false,
                ring_timeout_secs: Some(40),
                caller: None,
                ..
            })
        ));
    }

    #[test]
    fn invisible_formatting_is_stripped_from_plugin_text() {
        let info = PluginInfo::from_ready(
            Some("Safe\u{202E}gnp.exe\u{200B}".into()),
            Some("\u{200B}\u{FEFF}".into()),
            None,
        );
        assert_eq!(info.name.as_deref(), Some("Safegnp.exe"));
        assert_eq!(info.description, None);
    }

    #[test]
    fn plugin_self_descriptions_are_cleaned_for_display() {
        let info = PluginInfo::from_ready(
            Some("  Teams\ncall\talerts ".into()),
            Some("x".repeat(400)),
            Some("http://example.com".into()),
        );
        assert_eq!(info.name.as_deref(), Some("Ring alerts"));
        assert_eq!(info.description.as_ref().unwrap().chars().count(), 300);
        assert!(info.description.unwrap().ends_with('…'));
        assert_eq!(info.docs_url, None);
        let info = PluginInfo::from_ready(None, Some(" \n ".into()), Some("https://a.b/c".into()));
        assert_eq!(info.description, None);
        assert_eq!(info.docs_url.as_deref(), Some("https://a.b/c"));
    }

    #[test]
    fn docs_url_with_userinfo_is_dropped() {
        let info = |url: &str| PluginInfo::from_ready(None, None, Some(url.into())).docs_url;
        assert_eq!(info("https://user@example.com/x"), None);
        assert_eq!(info("https://github.com@evil.example/x"), None);
        assert_eq!(info("https://u:p@example.com"), None);
        assert_eq!(info("ftp://example.com/x"), None);
        assert_eq!(
            info("https://example.com/a@b").as_deref(),
            Some("https://example.com/a@b")
        );
    }

    #[test]
    fn an_error_with_a_number_message_still_parses() {
        assert_eq!(
            PluginMessage::parse(r#"{"v":1,"type":"error","message":5,"account":[1]}"#),
            Some(PluginMessage::Error {
                message: None,
                account: None
            })
        );
        assert_eq!(
            PluginMessage::parse(r#"{"v":1,"type":"signed_in","account":3,"username":{}}"#),
            Some(PluginMessage::SignedIn {
                account: None,
                username: None
            })
        );
        assert_eq!(
            PluginMessage::parse(r#"{"v":1,"type":"listening","account":"o.t","username":4}"#),
            Some(PluginMessage::Listening {
                account: "o.t".into(),
                username: None
            })
        );
        assert_eq!(
            PluginMessage::parse(
                r#"{"v":1,"type":"sign_in_required","verification_uri":"u","user_code":"c","method":1,"account":false}"#
            ),
            Some(PluginMessage::SignInRequired {
                method: None,
                verification_uri: "u".into(),
                user_code: "c".into(),
                account: None,
                username: None
            })
        );
        // A mistyped required field drops just that message.
        assert_eq!(
            PluginMessage::parse(r#"{"v":1,"type":"listening","account":5}"#),
            None
        );
    }

    #[test]
    fn account_ids_have_a_fixed_shape() {
        assert!(account_id("o-1.t-2").is_some());
        for bad in ["", "a", "a.", ".b", "a.b.c", "a b.c", "a\n.b", "a_b.c"] {
            assert_eq!(account_id(bad), None, "{bad:?}");
        }
        assert_eq!(account_id(&format!("{}.t", "a".repeat(99))), None);
    }

    fn opt(v: serde_json::Value) -> Vec<OptionSpec> {
        clean_options(vec![v])
    }

    #[test]
    fn a_text_option_cleans_to_a_spec() {
        let specs = opt(serde_json::json!({"id":"ip","kind":"text","label":"Bulb address"}));
        assert_eq!(
            specs,
            vec![OptionSpec {
                id: "ip".into(),
                kind: OptionKind::Text,
                label: "Bulb address".into(),
                default: None,
                choices: vec![],
            }]
        );
    }

    #[test]
    fn options_with_bad_ids_kinds_or_defaults_are_dropped() {
        use serde_json::json;
        let raw = vec![
            json!({"id":"Bad Id","kind":"text","label":"x"}),
            json!({"id":"a","kind":"script","label":"x"}),
            json!({"id":"","kind":"text","label":"x"}),
            json!({"id": "a".repeat(33),"kind":"text","label":"x"}),
            json!({"id":"b","kind":"toggle","label":"x","default":"yes"}),
            json!({"id":"c","kind":"text","label":"x","default":3}),
            json!({"id":"d","kind":"text","label":"  "}),
            json!({"id":7,"kind":"text","label":"x"}),
            json!("nope"),
        ];
        assert!(clean_options(raw).is_empty());
        let ok = opt(json!({"id":"t","kind":"toggle","label":"T","default":true}));
        assert_eq!(ok[0].default, Some(json!(true)));
    }

    #[test]
    fn more_than_16_options_keep_the_first_16() {
        let raw = (0..20)
            .map(|i| serde_json::json!({"id":format!("o{i}"),"kind":"toggle","label":"x"}))
            .collect();
        let specs = clean_options(raw);
        assert_eq!(specs.len(), 16);
        assert_eq!(specs[15].id, "o15");
    }

    #[test]
    fn duplicate_option_ids_keep_the_first() {
        let specs = clean_options(vec![
            serde_json::json!({"id":"a","kind":"text","label":"First"}),
            serde_json::json!({"id":"a","kind":"toggle","label":"Second"}),
        ]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].label, "First");
    }

    #[test]
    fn choice_without_choices_or_over_12_is_dropped() {
        use serde_json::json;
        assert!(opt(json!({"id":"c","kind":"choice","label":"x"})).is_empty());
        assert!(opt(json!({"id":"c","kind":"choice","label":"x","choices":[]})).is_empty());
        let many: Vec<_> = (0..13)
            .map(|i| json!({"value":format!("v{i}"),"label":"L"}))
            .collect();
        assert!(opt(json!({"id":"c","kind":"choice","label":"x","choices":many})).is_empty());
        let twelve: Vec<_> = (0..12)
            .map(|i| json!({"value":format!("v{i}"),"label":"L"}))
            .collect();
        let specs = opt(json!({"id":"c","kind":"choice","label":"x","choices":twelve}));
        assert_eq!(specs[0].choices.len(), 12);
        let bad_default = json!({"id":"c","kind":"choice","label":"x","default":"zz",
            "choices":[{"value":"a","label":"A"}]});
        assert!(opt(bad_default).is_empty());
    }

    #[test]
    fn secret_with_a_default_is_dropped() {
        use serde_json::json;
        assert!(opt(json!({"id":"s","kind":"secret","label":"x","default":"p"})).is_empty());
        let plain_secret = opt(json!({"id":"s","kind":"secret","label":"x"}));
        assert_eq!(plain_secret[0].kind, OptionKind::Secret);
    }

    #[test]
    fn option_labels_lose_invisible_characters() {
        let specs =
            opt(serde_json::json!({"id":"a","kind":"text","label":"Sa\u{202E}fe\n\u{200B}x"}));
        assert_eq!(specs[0].label, "Safe x");
    }

    #[test]
    fn hostile_text_and_types_in_ready_are_neutralized() {
        use serde_json::json;
        // Wrong types in ready's fields do not drop the whole message.
        let ready = PluginMessage::parse(
            r#"{"v":1,"type":"ready","capabilities":["presence_display",7],"name":5,"description":[],"docs_url":{},"forgotten":"x"}"#,
        );
        assert!(matches!(
            ready,
            Some(PluginMessage::Ready { ref capabilities, name: None, description: None, docs_url: None, ref forgotten, .. })
                if capabilities == &["presence_display".to_owned()] && forgotten.is_empty()
        ));
        // Hidden characters never leave a doubled space or survive in labels.
        let specs = opt(json!({"id":"a","kind":"text","label":"a \u{200B} b\u{E0041}\u{3164}c"}));
        assert_eq!(specs[0].label, "a bc");
        // Defaults and choice values with hidden characters are dropped.
        assert!(opt(json!({"id":"a","kind":"text","label":"x","default":"a\u{202E}b"})).is_empty());
        let hidden = json!({"id":"c","kind":"choice","label":"x",
            "choices":[{"value":"a\u{200B}","label":"A"}]});
        assert!(opt(hidden).is_empty());
        // Wrong types for choices and values are dropped, not panicked on.
        assert!(opt(json!({"id":"c","kind":"choice","label":"x","choices":"abc"})).is_empty());
        assert!(
            opt(
                json!({"id":"c","kind":"choice","label":"x","choices":[1,{"value":2,"label":"x"}]})
            )
            .is_empty()
        );
        // Duplicate choice values drop the option.
        let dup = json!({"id":"c","kind":"choice","label":"x",
            "choices":[{"value":"a","label":"A"},{"value":"a","label":"B"}]});
        assert!(opt(dup).is_empty());
    }

    #[test]
    fn a_ready_without_needs_or_options_still_parses() {
        let ready = PluginMessage::parse(r#"{"v":1,"type":"ready"}"#);
        assert!(matches!(
            ready,
            Some(PluginMessage::Ready { ref needs, ref options, .. })
                if needs.is_empty() && options.is_empty()
        ));
        let odd = PluginMessage::parse(r#"{"v":1,"type":"ready","options":"x","needs":3}"#);
        assert!(matches!(
            odd,
            Some(PluginMessage::Ready { ref needs, ref options, .. })
                if needs.is_empty() && options.is_empty()
        ));
    }

    #[test]
    fn options_and_presence_fixtures_match() {
        let options = HostMessage::Options {
            values: [
                ("brightness".to_owned(), serde_json::json!("high")),
                ("flash".to_owned(), serde_json::json!(true)),
                ("ip".to_owned(), serde_json::json!("192.168.1.20")),
                ("token".to_owned(), serde_json::json!("s3cret")),
            ]
            .into(),
        };
        assert_eq!(value(&options.to_line()), value(&fixture("options")));
        let presence = HostMessage::Presence {
            availability: "Busy".into(),
            activity: Some("InACall".into()),
        };
        assert_eq!(value(&presence.to_line()), value(&fixture("presence")));
        let none = HostMessage::Presence {
            availability: "Away".into(),
            activity: None,
        };
        assert!(value(&none.to_line()).get("activity").is_none());
    }

    #[test]
    fn the_ready_options_fixture_parses_and_cleans() {
        let Some(PluginMessage::Ready { needs, options, .. }) =
            PluginMessage::parse(&fixture("ready_options"))
        else {
            panic!("ready_options is a ready");
        };
        assert_eq!(needs, vec!["presence".to_owned()]);
        let specs = clean_options(options);
        let kinds: Vec<_> = specs.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            [
                OptionKind::Text,
                OptionKind::Toggle,
                OptionKind::Choice,
                OptionKind::Secret
            ]
        );
        assert_eq!(specs[2].choices.len(), 2);
        assert_eq!(specs[2].default, Some(serde_json::json!("high")));
    }
}
