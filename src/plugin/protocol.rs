//! The plugin protocol: newline-delimited JSON, each message with `"v": 1`
//! and a `type` (docs/calling.md#protocol). The same messages live as test
//! fixtures in `tests/fixtures/plugin-protocol/`, copied unchanged into the
//! plugin's repository, so the two sides cannot drift apart unnoticed.
//!
//! Everything here is plugin-neutral: identities are Entra object and tenant
//! ids and sign-in names. How a plugin learns them is its own business.

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
        #[serde(default)]
        capabilities: Vec<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        docs_url: Option<String>,
        /// The `hello.forget` accounts whose sign-ins the plugin cleared.
        #[serde(default)]
        forgotten: Vec<String>,
    },
    SignInRequired {
        /// How to sign in. Absent means `device_code` (older plugins).
        #[serde(default)]
        method: Option<String>,
        verification_uri: String,
        user_code: String,
        #[serde(default)]
        account: Option<String>,
        #[serde(default)]
        username: Option<String>,
    },
    SignedIn {
        #[serde(default)]
        account: Option<String>,
        #[serde(default)]
        username: Option<String>,
    },
    Listening {
        account: String,
        #[serde(default)]
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
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
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
                .filter(|url| !url.chars().any(|c| c.is_whitespace() || c.is_control())),
        }
    }
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
    )
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
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect();
    let text: String = words
        .join(" ")
        .chars()
        .filter(|c| !is_invisible(*c))
        .collect();
    let text = text.trim().to_owned();
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
}
