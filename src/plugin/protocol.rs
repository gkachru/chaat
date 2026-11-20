//! The plugin protocol: newline-delimited JSON, each message with `"v": 1`
//! and a `type` (docs/plugins.md). The same messages live as test
//! fixtures in `tests/fixtures/plugin-protocol/`, copied unchanged into the
//! plugin's repository, so the two sides cannot drift apart unnoticed.
//!
//! Everything here is plugin-neutral: identities are Entra object and tenant
//! ids and sign-in names. How a plugin learns them is its own business.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::options;

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
    /// The person clicked an `action` option's button. Only sent to a running
    /// plugin that has said `ready`.
    Action {
        id: String,
    },
    /// The channels Chaat accepts pushes on, one per account; replaces the
    /// previous list. Only sent to a plugin that needs `push`.
    PushChannels {
        channels: Vec<String>,
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
    /// How an option or action is doing. A `state` other than `ok`, `error`
    /// or `working` drops the message.
    OptionStatus {
        id: String,
        state: StatusState,
        #[serde(default, deserialize_with = "lenient_text")]
        message: Option<String>,
    },
    /// Replaces one `choice` option's choices; raw, see `clean_choices`. A
    /// non-array reads as none.
    OptionsUpdate {
        id: String,
        #[serde(default, deserialize_with = "lenient_array")]
        choices: Vec<serde_json::Value>,
    },
    /// The plugin's receiver is reachable at this public address.
    PushReady { url: String },
    /// One notification body Graph posted to the receiver, on the channel
    /// part of the path it arrived on. Untrusted until Chaat checks it.
    Push { channel: String, body: String },
    /// The receiver stopped or lost its address.
    PushDown {
        #[serde(default, deserialize_with = "lenient_text")]
        reason: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// The state in an `option_status` message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Ok,
    Error,
    Working,
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
            docs_url: docs_url.filter(|url| is_https_url(url)),
        }
    }
}

/// An `https://` address with a host, at most 300 characters, no whitespace
/// or control characters, and no user info.
pub(super) fn is_https_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // The host without a port: `https://:443/` has none.
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    url.len() <= 300
        && !host.is_empty()
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || is_invisible(c))
        && !has_userinfo(url)
}

/// The origin (`https://host[:port]`, no trailing slash) of a plugin's push
/// address: an `is_https_url` address with no path beyond `/`, no query, no
/// fragment. Anything else is `None`.
pub fn push_origin(url: &str) -> Option<String> {
    if !is_https_url(url) {
        return None;
    }
    let rest = url.strip_prefix("https://")?;
    let (authority, tail) = match rest.find(['/', '?', '#']) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    (tail.is_empty() || tail == "/")
        .then(|| authority_ok(authority))?
        .then(|| format!("https://{authority}"))
}

/// A plain host (letters, digits, dots, hyphens) or bracketed IPv6 literal,
/// then an optional `:port` of 1 to 5 digits, 1 to 65535.
fn authority_ok(authority: &str) -> bool {
    let (host_ok, port) = if let Some(rest) = authority.strip_prefix('[') {
        let Some((v6, after)) = rest.split_once(']') else {
            return false;
        };
        let v6_ok = !v6.is_empty()
            && v6
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
        match after {
            "" => (v6_ok, None),
            _ => (v6_ok, after.strip_prefix(':').or(Some("!"))),
        }
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        let ok = host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        (ok, port)
    };
    let port_ok = port.is_none_or(|port| {
        (1..=5).contains(&port.len())
            && port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p))
    });
    host_ok && port_ok
}

/// A push channel: 16 to 64 characters of `A-Za-z0-9-_`.
pub fn is_push_channel(text: &str) -> bool {
    (16..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
/// The longest `text` value, in characters.
pub(super) const MAX_TEXT: usize = 200;
/// The longest `multiline` value, in characters.
pub(super) const MAX_MULTILINE: usize = 2000;

/// What kind of control an option is drawn as.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OptionKind {
    Toggle,
    Choice,
    #[default]
    Text,
    Secret,
    /// A number within `min` and `max`.
    Number,
    /// `#rrggbb`.
    Color,
    /// Text of several lines.
    Multiline,
    /// A button with a label; no value.
    Action,
    /// A kind this Chaat doesn't know (the cleaned name, at most 32
    /// characters). Kept so the person sees that the setting exists.
    Unsupported {
        kind: String,
    },
}

/// What a `text` option's value must look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFormat {
    Ip,
    Hostname,
    UrlHttps,
    Email,
}

/// One choice of a `choice` option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionChoice {
    pub value: String,
    pub label: String,
}

/// Show an option only while another option has this value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleWhen {
    /// The other option's id.
    pub option: String,
    /// A JSON string or bool, compared for plain equality.
    pub equals: serde_json::Value,
}

/// A plugin-described setting, cleaned for display. `Default` is a blank
/// text option, for building one with `..Default::default()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionSpec {
    /// `[a-z0-9_]{1,32}`.
    pub id: String,
    pub kind: OptionKind,
    pub label: String,
    /// A bool for `toggle`, a string for `text`, `multiline` and `color`, one
    /// of the choice values for `choice`, a number for `number`; never
    /// present for `secret` or `action`.
    pub default: Option<serde_json::Value>,
    pub choices: Vec<OptionChoice>,
    /// Plain text, at most 200 characters.
    pub help: Option<String>,
    /// Plain text, at most 60 characters.
    pub placeholder: Option<String>,
    /// Plain text, at most 40 characters: the heading it is grouped under.
    pub section: Option<String>,
    /// Folded into the section's "Advanced" area.
    pub advanced: bool,
    /// A secret counts as set once saved. Never set for `action` and
    /// unsupported kinds.
    pub required: bool,
    pub visible_when: Option<VisibleWhen>,
    /// `number` only; finite, and `min <= max`.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// `number` only; greater than 0.
    pub step: Option<f64>,
    /// `number` only; plain text, at most 12 characters.
    pub unit: Option<String>,
    /// `text` only.
    pub format: Option<TextFormat>,
    /// `text` and `multiline`: characters. A value shorter than this is
    /// refused unless it is empty (an empty value is "not set").
    pub min_length: Option<usize>,
    /// `text` (at most 200) and `multiline` (at most 2000).
    pub max_length: Option<usize>,
}

// The only floats are `min`, `max` and `step`, which `clean_options` takes
// from JSON numbers: never NaN, so equality is reflexive.
impl Eq for OptionSpec {}

/// An option or choice id from a plugin: `[a-z0-9_]{1,32}`.
pub fn option_id(text: &str) -> Option<String> {
    let ok = (1..=32).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    ok.then(|| text.to_owned())
}

/// Cleans untrusted `ready.options`: drops what is malformed (bad id, kind,
/// label, default or limits), keeps the first of a duplicate id and the first
/// 16.
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

/// Cleans the choices of a `choice` option (in `ready`, or in
/// `options_update`): at most 12, unique values, plain labels. `None` when
/// any entry is malformed or there are too many. An empty list is fine here;
/// `ready` additionally refuses a choice option with none.
pub fn clean_choices(list: &[serde_json::Value]) -> Option<Vec<OptionChoice>> {
    if list.len() > MAX_CHOICES {
        return None;
    }
    let mut choices: Vec<OptionChoice> = Vec::new();
    for entry in list {
        let value = entry.get("value")?.as_str()?;
        let label = plain(entry.get("label")?.as_str()?, 60)?;
        if value.is_empty()
            || value.chars().count() > 64
            || value.chars().any(|c| c.is_control() || is_invisible(c))
            || choices.iter().any(|c| c.value == value)
        {
            return None;
        }
        choices.push(OptionChoice {
            value: value.to_owned(),
            label,
        });
    }
    Some(choices)
}

/// A plain-text field of at most `max` characters; `None` for anything else.
fn text_field(item: &serde_json::Value, key: &str, max: usize) -> Option<String> {
    plain(item.get(key)?.as_str()?, max)
}

/// `true` only for a JSON `true`.
fn flag(item: &serde_json::Value, key: &str) -> bool {
    item.get(key) == Some(&serde_json::Value::Bool(true))
}

/// An absent or null field is `Ok(None)`; a number is `Ok(Some)`; anything
/// else is an error, which drops the option.
fn number_field(item: &serde_json::Value, key: &str) -> Result<Option<f64>, ()> {
    match item.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => {
            n.as_f64().filter(|f| f.is_finite()).map(Some).ok_or(())
        }
        Some(_) => Err(()),
    }
}

/// Like `number_field`, for a count of characters.
fn length_field(item: &serde_json::Value, key: &str) -> Result<Option<usize>, ()> {
    match item.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map(|n| Some(usize::try_from(n).unwrap_or(usize::MAX)))
            .ok_or(()),
        Some(_) => Err(()),
    }
}

fn clean_visible_when(raw: Option<&serde_json::Value>, own: &str) -> Option<VisibleWhen> {
    let object = raw?.as_object()?;
    let option = option_id(object.get("option")?.as_str()?)?;
    if option == own {
        return None;
    }
    let equals = object.get("equals")?;
    let ok = match equals {
        serde_json::Value::Bool(_) => true,
        serde_json::Value::String(text) => {
            text.chars().count() <= 64 && !text.chars().any(|c| c.is_control() || is_invisible(c))
        }
        _ => false,
    };
    ok.then(|| VisibleWhen {
        option,
        equals: equals.clone(),
    })
}

fn clean_option(item: &serde_json::Value) -> Option<OptionSpec> {
    let id = option_id(item.get("id")?.as_str()?)?;
    let kind = match item.get("kind")?.as_str()? {
        "toggle" => OptionKind::Toggle,
        "choice" => OptionKind::Choice,
        "text" => OptionKind::Text,
        "secret" => OptionKind::Secret,
        "number" => OptionKind::Number,
        "color" => OptionKind::Color,
        "multiline" => OptionKind::Multiline,
        "action" => OptionKind::Action,
        other => OptionKind::Unsupported {
            kind: plain(other, 32)?,
        },
    };
    let label = plain(item.get("label")?.as_str()?, 60)?;
    let can_be_required = !matches!(kind, OptionKind::Action | OptionKind::Unsupported { .. });
    let mut spec = OptionSpec {
        visible_when: clean_visible_when(item.get("visible_when"), &id),
        help: text_field(item, "help", 200),
        placeholder: text_field(item, "placeholder", 60),
        section: text_field(item, "section", 40),
        advanced: flag(item, "advanced"),
        required: can_be_required && flag(item, "required"),
        id,
        kind,
        label,
        ..OptionSpec::default()
    };
    match spec.kind {
        OptionKind::Choice => {
            let list = item.get("choices")?.as_array()?;
            if list.is_empty() {
                return None;
            }
            spec.choices = clean_choices(list)?;
        }
        OptionKind::Number => {
            spec.min = number_field(item, "min").ok()?;
            spec.max = number_field(item, "max").ok()?;
            spec.step = number_field(item, "step").ok()?;
            spec.unit = text_field(item, "unit", 12);
            let ordered = match (spec.min, spec.max) {
                (Some(min), Some(max)) => min <= max,
                _ => true,
            };
            if !ordered || spec.step.is_some_and(|step| step <= 0.0) {
                return None;
            }
        }
        OptionKind::Text | OptionKind::Multiline => {
            let cap = if spec.kind == OptionKind::Text {
                MAX_TEXT
            } else {
                MAX_MULTILINE
            };
            spec.min_length = length_field(item, "min_length").ok()?;
            spec.max_length = length_field(item, "max_length")
                .ok()?
                .map(|max| max.min(cap));
            if spec.max_length == Some(0)
                || spec
                    .min_length
                    .is_some_and(|min| min > spec.max_length.unwrap_or(cap))
            {
                return None;
            }
            if spec.kind == OptionKind::Text {
                spec.format = match item.get("format").and_then(serde_json::Value::as_str) {
                    Some("ip") => Some(TextFormat::Ip),
                    Some("hostname") => Some(TextFormat::Hostname),
                    Some("url_https") => Some(TextFormat::UrlHttps),
                    Some("email") => Some(TextFormat::Email),
                    _ => None,
                };
            }
        }
        _ => {}
    }
    let default = item.get("default").filter(|v| !v.is_null());
    spec.default = match (&spec.kind, default) {
        (_, None) | (OptionKind::Unsupported { .. }, _) => None,
        // A secret never has a default, and a button has no value.
        (OptionKind::Secret | OptionKind::Action, Some(_)) => return None,
        (_, Some(value)) => Some(options::check_value(&spec, value).ok()?),
    };
    Some(spec)
}

/// Invisible formatting characters (direction overrides, zero-width marks)
/// that could make a plugin's text read differently from what it says.
pub(super) fn is_invisible(c: char) -> bool {
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
    fn push_origin_accepts_only_bare_https_origins() {
        assert_eq!(
            push_origin("https://a-b.trycloudflare.com/").as_deref(),
            Some("https://a-b.trycloudflare.com")
        );
        assert_eq!(
            push_origin("https://relay.example.com:8443").as_deref(),
            Some("https://relay.example.com:8443")
        );
        assert_eq!(
            push_origin("https://[::1]:8443/").as_deref(),
            Some("https://[::1]:8443")
        );
        assert_eq!(
            push_origin("https://[2001:db8::1]").as_deref(),
            Some("https://[2001:db8::1]")
        );
        let long = format!("https://{}.example", "a".repeat(300));
        for bad in [
            "https://u@h.example",
            "https://h.example/x",
            "https://h.example//",
            "https://h.example/?q=1",
            "https://h.example?q=1",
            "https://h.example/#f",
            "https://h.example#f",
            "https://h.exam\u{200b}ple",
            "https://h.example ",
            "https://h.example\n",
            "http://h.example",
            "",
            long.as_str(),
        ] {
            assert_eq!(push_origin(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn push_channels_are_16_to_64_url_safe_characters() {
        assert!(is_push_channel(&"a".repeat(16)));
        assert!(is_push_channel(&"A-_9".repeat(16)));
        assert!(!is_push_channel(&"a".repeat(15)));
        assert!(!is_push_channel(&"a".repeat(65)));
        assert!(!is_push_channel(&format!("{}/b", "a".repeat(20))));
        assert!(!is_push_channel(&format!("{}\u{e9}", "a".repeat(20))));
        assert!(!is_push_channel(&format!("{}\n", "a".repeat(20))));
    }

    #[test]
    fn mistyped_push_fields_drop_the_message() {
        for line in [
            r#"{"v":1,"type":"push_ready","url":5}"#,
            r#"{"v":1,"type":"push_ready"}"#,
            r#"{"v":1,"type":"push","channel":"c"}"#,
            r#"{"v":1,"type":"push","channel":"c","body":{}}"#,
        ] {
            assert_eq!(PluginMessage::parse(line), None, "{line}");
        }
        assert_eq!(
            PluginMessage::parse(r#"{"v":1,"type":"push_down","reason":7}"#),
            Some(PluginMessage::PushDown { reason: None })
        );
    }

    #[test]
    fn what_chaat_sends_matches_the_fixtures() {
        // hello.json is an example, not the capability list: the real list
        // is `registry::KNOWN`, which offers `push`.
        assert!(crate::plugin::registry::KNOWN.contains(&"push"));
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
            ("action", HostMessage::Action { id: "find".into() }),
            (
                "push_channels",
                HostMessage::PushChannels {
                    channels: vec!["abcdefghijklmnopqrstuvwxyzABCDEFGHIJ0123456".into()],
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
            PluginMessage::parse(&fixture("option_status")),
            Some(PluginMessage::OptionStatus {
                id: "find".into(),
                state: StatusState::Ok,
                message: Some("Found 2 bulbs".into()),
            })
        );
        let Some(PluginMessage::OptionsUpdate { id, choices }) =
            PluginMessage::parse(&fixture("options_update"))
        else {
            panic!("options_update");
        };
        assert_eq!(id, "bulb");
        assert_eq!(clean_choices(&choices).map(|c| c.len()), Some(2));
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
                ..OptionSpec::default()
            }]
        );
    }

    #[test]
    fn options_with_bad_ids_kinds_or_defaults_are_dropped() {
        use serde_json::json;
        let raw = vec![
            json!({"id":"Bad Id","kind":"text","label":"x"}),
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
        let kinds: Vec<_> = specs.iter().map(|s| s.kind.clone()).collect();
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

    use serde_json::json;

    fn one(v: serde_json::Value) -> OptionSpec {
        let mut specs = opt(v);
        assert_eq!(specs.len(), 1, "kept");
        specs.remove(0)
    }

    fn with_extra(base: serde_json::Value, extra: &serde_json::Value) -> serde_json::Value {
        let mut base = base;
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        base
    }

    #[test]
    fn display_fields_are_plain_and_limited() {
        let spec = one(json!({"id":"a","kind":"text","label":"A",
            "help":format!(" {}\n{} ", "h".repeat(150), "i".repeat(150)),
            "placeholder":"p".repeat(61),
            "section":"S".repeat(41),
            "advanced":true,"required":true}));
        assert_eq!(spec.help.as_ref().unwrap().chars().count(), 200);
        assert!(spec.help.as_ref().unwrap().ends_with('…'));
        assert_eq!(spec.placeholder.as_ref().unwrap().chars().count(), 60);
        assert_eq!(spec.section.as_ref().unwrap().chars().count(), 40);
        assert!(spec.advanced && spec.required);
        let exact = one(json!({"id":"a","kind":"text","label":"A",
            "help":"h".repeat(200),"placeholder":"p".repeat(60),"section":"S".repeat(40)}));
        assert_eq!(exact.help, Some("h".repeat(200)));
        assert_eq!(exact.placeholder, Some("p".repeat(60)));
        assert_eq!(exact.section, Some("S".repeat(40)));
        // Hidden characters go; text that is nothing, or isn't text, is none.
        let odd = one(
            json!({"id":"a","kind":"text","label":"A","help":"\u{202E} ","placeholder":5,
            "section":["x"],"advanced":"yes","required":1}),
        );
        assert_eq!((odd.help, odd.placeholder, odd.section), (None, None, None));
        assert!(!odd.advanced && !odd.required);
        let cleaned =
            one(json!({"id":"a","kind":"text","label":"A","section":" Co\u{200B}lors\n"}));
        assert_eq!(cleaned.section.as_deref(), Some("Colors"));
        let plain_option = one(json!({"id":"a","kind":"text","label":"A"}));
        assert_eq!(plain_option.help, None);
        assert!(!plain_option.advanced && !plain_option.required);
    }

    #[test]
    fn visible_when_needs_an_option_id_and_a_string_or_bool() {
        let with = |condition: serde_json::Value| {
            one(json!({"id":"a","kind":"text","label":"A","visible_when":condition})).visible_when
        };
        assert_eq!(
            with(json!({"option":"mode","equals":"manual"})),
            Some(VisibleWhen {
                option: "mode".into(),
                equals: json!("manual")
            })
        );
        assert_eq!(
            with(json!({"option":"on","equals":false})),
            Some(VisibleWhen {
                option: "on".into(),
                equals: json!(false)
            })
        );
        for bad in [
            json!({"option":"mode","equals":3}),
            json!({"option":"mode","equals":null}),
            json!({"option":"mode","equals":["a"]}),
            json!({"option":"mode"}),
            json!({"option":"Bad Id","equals":"x"}),
            json!({"option":5,"equals":"x"}),
            json!({"option":"mode","equals":"x".repeat(65)}),
            json!({"option":"mode","equals":"a\u{200B}"}),
            json!({"option":"a","equals":true}),
            json!("mode"),
        ] {
            assert_eq!(with(bad.clone()), None, "{bad}");
        }
    }

    #[test]
    fn number_options_keep_valid_limits_and_defaults() {
        let spec = one(
            json!({"id":"n","kind":"number","label":"N","min":10,"max":100,
            "step":5,"unit":"%","default":80}),
        );
        assert_eq!(spec.kind, OptionKind::Number);
        assert_eq!(
            (spec.min, spec.max, spec.step),
            (Some(10.0), Some(100.0), Some(5.0))
        );
        assert_eq!(spec.unit.as_deref(), Some("%"));
        assert_eq!(spec.default, Some(json!(80)));
        let open = one(json!({"id":"n","kind":"number","label":"N"}));
        assert_eq!(
            (open.min, open.max, open.step, open.unit),
            (None, None, None, None)
        );
        let same = one(json!({"id":"n","kind":"number","label":"N","min":3,"max":3,"default":3}));
        assert_eq!(same.default, Some(json!(3)));
        let long_unit = one(json!({"id":"n","kind":"number","label":"N","unit":"u".repeat(13)}));
        assert_eq!(long_unit.unit.as_ref().unwrap().chars().count(), 12);
        let fractional = one(json!({"id":"n","kind":"number","label":"N","min":-1.5,"step":0.25}));
        assert_eq!((fractional.min, fractional.step), (Some(-1.5), Some(0.25)));
    }

    #[test]
    fn number_options_with_bad_limits_or_defaults_are_dropped() {
        for bad in [
            json!({"min":5,"max":4}),
            json!({"step":0}),
            json!({"step":-1}),
            json!({"min":"1"}),
            json!({"max":true}),
            json!({"step":"x"}),
            json!({"min":1,"max":10,"default":11}),
            json!({"min":1,"max":10,"default":0}),
            json!({"default":"5"}),
            json!({"default":true}),
        ] {
            let option = with_extra(json!({"id":"n","kind":"number","label":"N"}), &bad);
            assert!(opt(option).is_empty(), "{bad}");
        }
    }

    #[test]
    fn text_formats_and_lengths_are_read_and_checked() {
        let formats = [
            ("ip", TextFormat::Ip),
            ("hostname", TextFormat::Hostname),
            ("url_https", TextFormat::UrlHttps),
            ("email", TextFormat::Email),
        ];
        for (name, format) in formats {
            let spec = one(json!({"id":"t","kind":"text","label":"T","format":name}));
            assert_eq!(spec.format, Some(format), "{name}");
        }
        for odd in [json!("regex"), json!("IP"), json!(4), json!(null)] {
            let spec = one(json!({"id":"t","kind":"text","label":"T","format":odd}));
            assert_eq!(spec.format, None, "{odd}");
        }
        let limited =
            one(json!({"id":"t","kind":"text","label":"T","min_length":2,"max_length":10}));
        assert_eq!(
            (limited.min_length, limited.max_length),
            (Some(2), Some(10))
        );
        let clamped = one(json!({"id":"t","kind":"text","label":"T","max_length":5000}));
        assert_eq!(clamped.max_length, Some(200));
        // Only text has a format.
        let multiline = one(json!({"id":"t","kind":"multiline","label":"T","format":"ip"}));
        assert_eq!(multiline.format, None);
    }

    #[test]
    fn text_options_with_bad_limits_or_defaults_are_dropped() {
        for bad in [
            json!({"min_length":5,"max_length":4}),
            json!({"min_length":201}),
            json!({"max_length":0}),
            json!({"min_length":-1}),
            json!({"max_length":2.5}),
            json!({"max_length":"9"}),
            json!({"format":"ip","default":"not an ip"}),
            json!({"format":"hostname","default":"-bad-"}),
            json!({"format":"email","default":"nobody"}),
            json!({"format":"url_https","default":"http://example.com"}),
            json!({"max_length":3,"default":"abcd"}),
            json!({"min_length":3,"default":"ab"}),
            json!({"default":"line\nbreak"}),
        ] {
            let option = with_extra(json!({"id":"t","kind":"text","label":"T"}), &bad);
            assert!(opt(option).is_empty(), "{bad}");
        }
        let good = one(json!({"id":"t","kind":"text","label":"T","format":"ip","default":"::1"}));
        assert_eq!(good.default, Some(json!("::1")));
        // An empty default means "none", whatever the format.
        let empty = one(
            json!({"id":"t","kind":"text","label":"T","format":"ip","min_length":4,"default":""}),
        );
        assert_eq!(empty.default, Some(json!("")));
    }

    #[test]
    fn color_options_default_to_lowercase_hex() {
        let spec = one(json!({"id":"c","kind":"color","label":"C","default":"#D13438"}));
        assert_eq!(spec.kind, OptionKind::Color);
        assert_eq!(spec.default, Some(json!("#d13438")));
        for bad in ["d13438", "#d1343", "#d134388", "red", "#ggg000"] {
            assert!(
                opt(json!({"id":"c","kind":"color","label":"C","default":bad})).is_empty(),
                "{bad}"
            );
        }
        assert!(opt(json!({"id":"c","kind":"color","label":"C","default":5})).is_empty());
        assert_eq!(
            one(json!({"id":"c","kind":"color","label":"C"})).default,
            None
        );
    }

    #[test]
    fn multiline_options_allow_line_breaks_up_to_2000_characters() {
        let spec =
            one(json!({"id":"m","kind":"multiline","label":"M","default":"a\nb","max_length":900}));
        assert_eq!(spec.kind, OptionKind::Multiline);
        assert_eq!(spec.default, Some(json!("a\nb")));
        assert_eq!(spec.max_length, Some(900));
        let longest = "x".repeat(2000);
        let ok = one(json!({"id":"m","kind":"multiline","label":"M","default":longest}));
        assert_eq!(
            ok.default.as_ref().and_then(|d| d.as_str()).map(str::len),
            Some(2000)
        );
        let too_long = "x".repeat(2001);
        assert!(
            opt(json!({"id":"m","kind":"multiline","label":"M","default":too_long})).is_empty()
        );
        let clamped = one(json!({"id":"m","kind":"multiline","label":"M","max_length":9000}));
        assert_eq!(clamped.max_length, Some(2000));
        assert!(opt(json!({"id":"m","kind":"multiline","label":"M","min_length":2001})).is_empty());
    }

    #[test]
    fn an_action_has_a_label_and_no_default_or_requirement() {
        let spec = one(
            json!({"id":"find","kind":"action","label":"Find bulbs","required":true,
            "help":"Listens for bulbs."}),
        );
        assert_eq!(spec.kind, OptionKind::Action);
        assert!(!spec.required);
        assert_eq!(spec.default, None);
        assert_eq!(spec.help.as_deref(), Some("Listens for bulbs."));
        assert!(opt(json!({"id":"find","kind":"action","label":"Find","default":true})).is_empty());
        assert!(opt(json!({"id":"find","kind":"action","label":"  "})).is_empty());
    }

    #[test]
    fn an_unknown_kind_is_kept_not_dropped() {
        let spec = one(
            json!({"id":"dial","kind":"dial","label":"Dial","section":"Odd",
            "required":true,"default":3,"min":1}),
        );
        assert_eq!(
            spec.kind,
            OptionKind::Unsupported {
                kind: "dial".into()
            }
        );
        assert_eq!(spec.label, "Dial");
        assert_eq!(spec.section.as_deref(), Some("Odd"));
        assert!(!spec.required);
        assert_eq!((spec.default, spec.min), (None, None));
        // The kind name is plain text, at most 32 characters.
        let long_name = format!(" Fan\u{202E}cy{}", "y".repeat(60));
        let spec = one(json!({"id":"d","kind":long_name,"label":"D"}));
        let OptionKind::Unsupported { kind } = spec.kind else {
            panic!("unsupported");
        };
        assert_eq!(kind.chars().count(), 32);
        assert!(kind.starts_with("Fancy") && kind.ends_with('…'));
        // Nothing to name it by, a non-text kind, or a bad id or label: dropped.
        assert!(opt(json!({"id":"d","kind":" \u{200B}","label":"D"})).is_empty());
        assert!(opt(json!({"id":"d","kind":7,"label":"D"})).is_empty());
        assert!(opt(json!({"id":"D","kind":"dial","label":"D"})).is_empty());
        assert!(opt(json!({"id":"d","kind":"dial","label":" "})).is_empty());
    }

    #[test]
    fn the_16_option_and_12_choice_limits_still_hold_with_new_kinds() {
        let raw = (0..20)
            .map(|i| json!({"id":format!("o{i}"),"kind":"dial","label":"x"}))
            .collect();
        assert_eq!(clean_options(raw).len(), 16);
        let twelve: Vec<_> = (0..12)
            .map(|i| json!({"value":format!("v{i}"),"label":"L"}))
            .collect();
        assert_eq!(clean_choices(&twelve).map(|c| c.len()), Some(12));
        let thirteen: Vec<_> = (0..13)
            .map(|i| json!({"value":format!("v{i}"),"label":"L"}))
            .collect();
        assert_eq!(clean_choices(&thirteen), None);
        assert_eq!(clean_choices(&[]), Some(vec![]));
    }

    #[test]
    fn option_ids_have_a_fixed_shape() {
        assert_eq!(option_id("a_1").as_deref(), Some("a_1"));
        for bad in ["", "A", "a-b", "a b", "é", &"a".repeat(33)] {
            assert_eq!(option_id(bad), None, "{bad:?}");
        }
        assert!(option_id(&"a".repeat(32)).is_some());
    }

    #[test]
    fn option_status_and_options_update_parse_leniently() {
        for (state, expected) in [
            ("ok", StatusState::Ok),
            ("error", StatusState::Error),
            ("working", StatusState::Working),
        ] {
            let line = json!({"v":1,"type":"option_status","id":"x","state":state}).to_string();
            assert_eq!(
                PluginMessage::parse(&line),
                Some(PluginMessage::OptionStatus {
                    id: "x".into(),
                    state: expected,
                    message: None
                })
            );
        }
        let odd = json!({"v":1,"type":"option_status","id":"x","state":"ok","message":[1]});
        assert!(matches!(
            PluginMessage::parse(&odd.to_string()),
            Some(PluginMessage::OptionStatus { message: None, .. })
        ));
        let bad_state = json!({"v":1,"type":"option_status","id":"x","state":"fine"});
        assert_eq!(PluginMessage::parse(&bad_state.to_string()), None);
        let no_id = json!({"v":1,"type":"option_status","state":"ok"});
        assert_eq!(PluginMessage::parse(&no_id.to_string()), None);
        let numeric_id = json!({"v":1,"type":"options_update","id":5,"choices":[]});
        assert_eq!(PluginMessage::parse(&numeric_id.to_string()), None);
        let not_a_list = json!({"v":1,"type":"options_update","id":"x","choices":"a"});
        assert_eq!(
            PluginMessage::parse(&not_a_list.to_string()),
            Some(PluginMessage::OptionsUpdate {
                id: "x".into(),
                choices: vec![]
            })
        );
        let missing = json!({"v":1,"type":"options_update","id":"x"});
        assert!(matches!(
            PluginMessage::parse(&missing.to_string()),
            Some(PluginMessage::OptionsUpdate { ref choices, .. }) if choices.is_empty()
        ));
    }

    #[test]
    fn an_action_message_is_one_line_with_the_option_id() {
        let line = HostMessage::Action {
            id: "test_light".into(),
        }
        .to_line();
        assert_eq!(
            value(&line),
            json!({"v":1,"type":"action","id":"test_light"})
        );
        assert!(line.ends_with('\n') && line.trim_end().lines().count() == 1);
    }

    #[test]
    fn the_rich_ready_fixture_cleans_to_every_new_kind_and_field() {
        let Some(PluginMessage::Ready { options, .. }) =
            PluginMessage::parse(&fixture("ready_options_rich"))
        else {
            panic!("ready_options_rich is a ready");
        };
        assert_eq!(options.len(), 14);
        let specs = clean_options(options);
        assert_eq!(specs.len(), 14, "every option survives cleaning");
        let by = |id: &str| specs.iter().find(|s| s.id == id).expect(id);
        assert_eq!(by("find").kind, OptionKind::Action);
        assert_eq!(by("find").section.as_deref(), Some("Bulb"));
        assert!(by("find").help.is_some());
        assert_eq!(by("key").kind, OptionKind::Secret);
        assert!(by("key").required);
        assert!(by("key").placeholder.is_some());
        assert!(by("mode").advanced);
        assert_eq!(by("address").format, Some(TextFormat::Ip));
        assert_eq!(
            by("address").visible_when,
            Some(VisibleWhen {
                option: "mode".into(),
                equals: json!("manual")
            })
        );
        assert_eq!(by("host").format, Some(TextFormat::Hostname));
        assert_eq!(
            (by("host").min_length, by("host").max_length),
            (Some(1), Some(63))
        );
        assert_eq!(by("contact").format, Some(TextFormat::Email));
        assert_eq!(by("docs").format, Some(TextFormat::UrlHttps));
        assert_eq!(by("busy").kind, OptionKind::Color);
        assert_eq!(by("busy").default, Some(json!("#d13438")));
        let brightness = by("brightness");
        assert_eq!(brightness.kind, OptionKind::Number);
        assert_eq!(
            (brightness.min, brightness.max, brightness.step),
            (Some(10.0), Some(100.0), Some(5.0))
        );
        assert_eq!(brightness.unit.as_deref(), Some("%"));
        assert_eq!(brightness.default, Some(json!(80)));
        assert_eq!(
            by("flash_rate").visible_when,
            Some(VisibleWhen {
                option: "flash".into(),
                equals: json!(true)
            })
        );
        assert_eq!(by("notes").kind, OptionKind::Multiline);
        assert_eq!(by("notes").max_length, Some(500));
        assert_eq!(by("notes").default, Some(json!("Line one\nLine two")));
        assert_eq!(
            by("dial").kind,
            OptionKind::Unsupported {
                kind: "dial".into()
            }
        );
    }

    #[test]
    fn https_urls_need_a_host_and_no_invisible_characters() {
        assert!(is_https_url("https://example.com:443/x"));
        assert!(!is_https_url("https://:443/x"));
        assert!(!is_https_url("https://exam\u{202E}ple.com/x"));
        assert!(!is_https_url("https://example.com/\u{200B}x"));
    }
}
