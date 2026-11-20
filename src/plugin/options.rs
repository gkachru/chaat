//! Checks on option values, for the settings a plugin describes
//! (`protocol::OptionSpec`). Pure: no I/O and no state. A value is checked
//! before it is stored or sent to the plugin; the errors are fixed Chaat
//! texts, safe to show under a field.
//!
//! An empty text value means "not set": it passes the format and length
//! checks, and `missing_required` is what asks for it.

use serde_json::{Map, Value};

use super::protocol::{
    MAX_MULTILINE, MAX_TEXT, OptionKind, OptionSpec, TextFormat, is_https_url, is_invisible,
};

/// The longest secret, in characters.
const MAX_SECRET: usize = 1000;

/// Checks `value` against the option's kind and limits and returns it
/// normalized: text and secrets are trimmed, a color is lowercased, line
/// breaks of multiline text become `\n`. Out of range is an error, never
/// clamped. The error says what to enter.
pub fn check_value(spec: &OptionSpec, value: &Value) -> Result<Value, String> {
    match &spec.kind {
        OptionKind::Toggle => match value {
            Value::Bool(_) => Ok(value.clone()),
            _ => Err("Choose on or off.".into()),
        },
        OptionKind::Choice => match value.as_str() {
            Some(text) if spec.choices.iter().any(|c| c.value == text) => Ok(value.clone()),
            _ => Err("Choose one of the listed options.".into()),
        },
        OptionKind::Text => {
            let text = value.as_str().ok_or("Enter text.")?.trim();
            if text.is_empty() {
                return Ok(Value::String(String::new()));
            }
            if text.chars().any(|c| c.is_control() || is_invisible(c)) {
                return Err("Use plain text only.".into());
            }
            check_length(spec, text, MAX_TEXT)?;
            if let Some(format) = spec.format {
                check_format(format, text)?;
            }
            Ok(Value::String(text.to_owned()))
        }
        OptionKind::Multiline => {
            let text = value.as_str().ok_or("Enter text.")?;
            let text = text.replace("\r\n", "\n");
            if text.trim().is_empty() {
                return Ok(Value::String(String::new()));
            }
            if text
                .chars()
                .any(|c| (c.is_control() && c != '\n' && c != '\t') || is_invisible(c))
            {
                return Err("Use plain text only.".into());
            }
            check_length(spec, &text, MAX_MULTILINE)?;
            Ok(Value::String(text))
        }
        OptionKind::Secret => {
            let text = value.as_str().ok_or("Enter text.")?.trim();
            if text.chars().any(|c| c.is_control() || is_invisible(c)) {
                return Err("Use plain text only.".into());
            }
            if text.chars().count() > MAX_SECRET {
                return Err(format!("Use at most {MAX_SECRET} characters."));
            }
            Ok(Value::String(text.to_owned()))
        }
        OptionKind::Number => check_number(spec, value),
        OptionKind::Color => {
            let text = value.as_str().ok_or("Enter a color like #ff8800.")?.trim();
            let digits = text.strip_prefix('#').unwrap_or_default();
            if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                Ok(Value::String(text.to_ascii_lowercase()))
            } else {
                Err("Enter a color like #ff8800.".into())
            }
        }
        OptionKind::Action => Err("This setting is a button; it has no value.".into()),
        OptionKind::Unsupported { .. } => Err("This setting needs a newer Chaat.".into()),
    }
}

fn check_length(spec: &OptionSpec, text: &str, cap: usize) -> Result<(), String> {
    let length = text.chars().count();
    let max = spec.max_length.unwrap_or(cap).min(cap);
    if length > max {
        return Err(format!("Use at most {max} characters."));
    }
    if let Some(min) = spec.min_length
        && length < min
    {
        return Err(format!("Use at least {min} characters."));
    }
    Ok(())
}

fn check_format(format: TextFormat, text: &str) -> Result<(), String> {
    let (ok, error) = match format {
        TextFormat::Ip => (
            text.parse::<std::net::IpAddr>().is_ok(),
            "Enter an IP address.",
        ),
        TextFormat::Hostname => (is_hostname(text), "Enter a host name."),
        TextFormat::UrlHttps => (is_https_url(text), "Enter an https:// address."),
        TextFormat::Email => (is_email(text), "Enter an email address."),
    };
    if ok { Ok(()) } else { Err(error.into()) }
}

/// RFC 1123: dot-separated labels of 1 to 63 letters, digits and hyphens,
/// none starting or ending with a hyphen, at most 253 characters in all.
fn is_hostname(text: &str) -> bool {
    text.len() <= 253
        && text.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// One `@`, a non-empty local part of at most 64 characters without
/// whitespace, and a host name after it.
fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.chars().count() <= 64
        && !local.chars().any(char::is_whitespace)
        && !domain.contains('@')
        && is_hostname(domain)
}

fn check_number(spec: &OptionSpec, value: &Value) -> Result<Value, String> {
    let error = || match (spec.min, spec.max) {
        (Some(min), Some(max)) => {
            format!("Enter a number from {} to {}.", show(min), show(max))
        }
        (Some(min), None) => format!("Enter a number of {} or more.", show(min)),
        (None, Some(max)) => format!("Enter a number of {} or less.", show(max)),
        (None, None) => "Enter a number.".to_owned(),
    };
    let number = match value {
        Value::Number(n) => n.as_f64().filter(|n| n.is_finite()).ok_or_else(error)?,
        _ => return Err(error()),
    };
    if spec.min.is_some_and(|min| number < min) || spec.max.is_some_and(|max| number > max) {
        return Err(error());
    }
    Ok(value.clone())
}

/// A number the way a person writes it: `10`, not `10.0`.
fn show(number: f64) -> String {
    format!("{number}")
}

/// Whether the option is shown, given the current values of the options
/// (defaults filled in; see `effective_values`). Plain equality on the other
/// option's value. A value that isn't there, because the id belongs to no
/// option (or to a secret, which isn't in `values`), hides nothing.
pub fn is_visible(spec: &OptionSpec, values: &Map<String, Value>) -> bool {
    match &spec.visible_when {
        None => true,
        Some(condition) => values
            .get(&condition.option)
            .is_none_or(|value| *value == condition.equals),
    }
}

/// The current value of each non-secret, non-action option: the stored value,
/// or else the default. Options with neither are left out.
pub fn effective_values(specs: &[OptionSpec], stored: &Map<String, Value>) -> Map<String, Value> {
    let mut values = Map::new();
    for spec in specs {
        if matches!(spec.kind, OptionKind::Secret | OptionKind::Action) {
            continue;
        }
        if let Some(value) = stored
            .get(&spec.id)
            .filter(|v| !v.is_null())
            .or(spec.default.as_ref())
        {
            values.insert(spec.id.clone(), value.clone());
        }
    }
    values
}

/// The ids of the required, visible options that have no value yet, in order.
/// A secret is set once it is among `saved_secrets`; text and multiline are
/// unset when empty or only whitespace; the default of the other kinds
/// counts as a value. Buttons and unsupported kinds are never required.
pub fn missing_required(
    specs: &[OptionSpec],
    values: &Map<String, Value>,
    saved_secrets: &[String],
) -> Vec<String> {
    let current = effective_values(specs, values);
    specs
        .iter()
        .filter(|spec| spec.required && is_visible(spec, &current))
        .filter(|spec| match &spec.kind {
            OptionKind::Action | OptionKind::Unsupported { .. } => false,
            OptionKind::Secret => !saved_secrets.contains(&spec.id),
            _ => match current.get(&spec.id) {
                None | Some(Value::Null) => true,
                Some(Value::String(text)) => text.trim().is_empty(),
                Some(_) => false,
            },
        })
        .map(|spec| spec.id.clone())
        .collect()
}

/// The defaults of the options in one section (`None` is the options with no
/// section), for "Reset to defaults". Secrets and buttons are left out, and
/// so are options without a default: a caller resetting a section also
/// clears the stored value of those.
pub fn defaults_for_section(specs: &[OptionSpec], section: Option<&str>) -> Map<String, Value> {
    specs
        .iter()
        .filter(|spec| spec.section.as_deref() == section)
        .filter(|spec| !matches!(spec.kind, OptionKind::Secret | OptionKind::Action))
        .filter_map(|spec| Some((spec.id.clone(), spec.default.clone()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::protocol::{OptionChoice, VisibleWhen, clean_options};
    use serde_json::json;

    fn spec(kind: OptionKind) -> OptionSpec {
        OptionSpec {
            id: "x".into(),
            kind,
            label: "X".into(),
            ..OptionSpec::default()
        }
    }

    fn text(format: Option<TextFormat>) -> OptionSpec {
        OptionSpec {
            format,
            ..spec(OptionKind::Text)
        }
    }

    fn number(min: Option<f64>, max: Option<f64>) -> OptionSpec {
        OptionSpec {
            min,
            max,
            ..spec(OptionKind::Number)
        }
    }

    #[test]
    fn toggles_and_choices_check_their_type_and_membership() {
        let toggle = spec(OptionKind::Toggle);
        assert_eq!(check_value(&toggle, &json!(false)), Ok(json!(false)));
        assert!(check_value(&toggle, &json!("true")).is_err());
        assert!(check_value(&toggle, &json!(1)).is_err());
        let choice = OptionSpec {
            choices: vec![OptionChoice {
                value: "a".into(),
                label: "A".into(),
            }],
            ..spec(OptionKind::Choice)
        };
        assert_eq!(check_value(&choice, &json!("a")), Ok(json!("a")));
        assert_eq!(
            check_value(&choice, &json!("b")),
            Err("Choose one of the listed options.".into())
        );
        assert!(check_value(&choice, &json!(1)).is_err());
    }

    #[test]
    fn numbers_must_be_in_range_and_are_never_clamped() {
        let both = number(Some(1.0), Some(100.0));
        assert_eq!(check_value(&both, &json!(1)), Ok(json!(1)));
        assert_eq!(check_value(&both, &json!(100)), Ok(json!(100)));
        assert_eq!(check_value(&both, &json!(55.5)), Ok(json!(55.5)));
        for bad in [json!(0), json!(101), json!(0.99), json!("5"), json!(null)] {
            assert_eq!(
                check_value(&both, &bad),
                Err("Enter a number from 1 to 100.".into()),
                "{bad}"
            );
        }
        assert_eq!(
            check_value(&number(Some(0.5), None), &json!(0)),
            Err("Enter a number of 0.5 or more.".into())
        );
        assert_eq!(
            check_value(&number(None, Some(9.0)), &json!(10)),
            Err("Enter a number of 9 or less.".into())
        );
        assert_eq!(
            check_value(&number(None, None), &json!("x")),
            Err("Enter a number.".into())
        );
        assert_eq!(
            check_value(&number(None, None), &json!(-1e9)),
            Ok(json!(-1e9))
        );
    }

    #[test]
    fn colors_are_six_hex_digits_and_saved_lowercase() {
        let color = spec(OptionKind::Color);
        assert_eq!(check_value(&color, &json!("#FF8800")), Ok(json!("#ff8800")));
        assert_eq!(
            check_value(&color, &json!(" #0a0B0c ")),
            Ok(json!("#0a0b0c"))
        );
        for bad in [
            json!("ff8800"),
            json!("#ff880"),
            json!("#ff88000"),
            json!("#gg8800"),
            json!("#fff"),
            json!(""),
            json!(5),
        ] {
            assert_eq!(
                check_value(&color, &bad),
                Err("Enter a color like #ff8800.".into()),
                "{bad}"
            );
        }
    }

    #[test]
    fn text_is_trimmed_plain_and_within_its_length() {
        let plain = text(None);
        assert_eq!(check_value(&plain, &json!("  hi ")), Ok(json!("hi")));
        assert_eq!(check_value(&plain, &json!("   ")), Ok(json!("")));
        assert!(check_value(&plain, &json!("a\nb")).is_err());
        assert!(check_value(&plain, &json!("a\u{202E}b")).is_err());
        assert!(check_value(&plain, &json!(3)).is_err());
        assert!(check_value(&plain, &json!("x".repeat(200))).is_ok());
        assert_eq!(
            check_value(&plain, &json!("x".repeat(201))),
            Err("Use at most 200 characters.".into())
        );
        let limited = OptionSpec {
            min_length: Some(3),
            max_length: Some(5),
            ..text(None)
        };
        assert!(check_value(&limited, &json!("abc")).is_ok());
        assert!(check_value(&limited, &json!("abcde")).is_ok());
        assert_eq!(
            check_value(&limited, &json!("ab")),
            Err("Use at least 3 characters.".into())
        );
        assert_eq!(
            check_value(&limited, &json!("abcdef")),
            Err("Use at most 5 characters.".into())
        );
        // Empty means "not set", not "too short".
        assert_eq!(check_value(&limited, &json!("")), Ok(json!("")));
    }

    #[test]
    fn multiline_allows_line_breaks_up_to_2000_characters() {
        let multi = spec(OptionKind::Multiline);
        assert_eq!(
            check_value(&multi, &json!("a\r\nb\tc")),
            Ok(json!("a\nb\tc"))
        );
        assert!(check_value(&multi, &json!("x".repeat(2000))).is_ok());
        assert_eq!(
            check_value(&multi, &json!("x".repeat(2001))),
            Err("Use at most 2000 characters.".into())
        );
        assert!(check_value(&multi, &json!("a\u{0}b")).is_err());
        assert!(check_value(&multi, &json!("a\u{200B}b")).is_err());
        assert_eq!(check_value(&multi, &json!(" \n ")), Ok(json!("")));
    }

    #[test]
    fn secrets_are_trimmed_text() {
        let secret = spec(OptionKind::Secret);
        assert_eq!(check_value(&secret, &json!(" k3y\n")), Ok(json!("k3y")));
        assert!(check_value(&secret, &json!("a\nb")).is_err());
        assert!(check_value(&secret, &json!(1)).is_err());
        assert!(check_value(&secret, &json!("x".repeat(1001))).is_err());
    }

    #[test]
    fn buttons_and_unknown_kinds_have_no_value() {
        assert!(check_value(&spec(OptionKind::Action), &json!(true)).is_err());
        let unknown = spec(OptionKind::Unsupported {
            kind: "dial".into(),
        });
        assert!(check_value(&unknown, &json!(1)).is_err());
    }

    #[test]
    fn ip_addresses_are_v4_or_v6() {
        let ip = text(Some(TextFormat::Ip));
        for good in ["192.168.1.20", "::1", "2001:db8::ff00:42:8329", "0.0.0.0"] {
            assert!(check_value(&ip, &json!(good)).is_ok(), "{good}");
        }
        for bad in [
            "192.168.1",
            "256.1.1.1",
            "host",
            "1.2.3.4:80",
            "1.2.3.4/24",
            "[::1]",
        ] {
            assert_eq!(
                check_value(&ip, &json!(bad)),
                Err("Enter an IP address.".into()),
                "{bad}"
            );
        }
        assert_eq!(check_value(&ip, &json!("")), Ok(json!("")));
    }

    #[test]
    fn host_names_follow_rfc_1123() {
        let host = text(Some(TextFormat::Hostname));
        for good in [
            "localhost",
            "bulb-1.local",
            "a.b.c",
            "1bulb",
            "x".repeat(63).as_str(),
        ] {
            assert!(check_value(&host, &json!(good)).is_ok(), "{good}");
        }
        let long_label = "x".repeat(64);
        for bad in [
            "-bulb",
            "bulb-",
            "a..b",
            ".a",
            "a.",
            "a b",
            "bu_lb",
            "bülb",
            "a:80",
            long_label.as_str(),
        ] {
            assert_eq!(
                check_value(&host, &json!(bad)),
                Err("Enter a host name.".into()),
                "{bad}"
            );
        }
        // RFC 1123 allows 253 characters; a text value holds at most 200.
        let name = |last: usize| format!("{0}.{0}.{0}.{1}", "a".repeat(63), "a".repeat(last));
        assert!(is_hostname(&name(61)) && name(61).len() == 253);
        assert!(!is_hostname(&name(62)));
        assert_eq!(
            check_value(&host, &json!(name(61))),
            Err("Use at most 200 characters.".into())
        );
    }

    #[test]
    fn emails_have_one_at_a_local_part_and_a_host() {
        let email = text(Some(TextFormat::Email));
        for good in ["ada@contoso.com", "a.b+c@localhost", "x@1.2"] {
            assert!(check_value(&email, &json!(good)).is_ok(), "{good}");
        }
        for bad in [
            "ada",
            "@contoso.com",
            "ada@",
            "a@b@c.com",
            "a b@c.com",
            "ada@-c.com",
            "ada@c..com",
            "ada@c.com.",
        ] {
            assert_eq!(
                check_value(&email, &json!(bad)),
                Err("Enter an email address.".into()),
                "{bad}"
            );
        }
        let long_local = format!("{}@c.com", "a".repeat(65));
        assert!(check_value(&email, &json!(long_local)).is_err());
    }

    #[test]
    fn https_urls_have_no_userinfo_or_spaces() {
        let url = text(Some(TextFormat::UrlHttps));
        for good in [
            "https://example.com",
            "https://a.b/c?d=e#f",
            "https://a.b:8443/x@y",
        ] {
            assert!(check_value(&url, &json!(good)).is_ok(), "{good}");
        }
        for bad in [
            "http://example.com",
            "example.com",
            "https://",
            "https:///path",
            "https://user@example.com",
            "https://a b",
            "ftp://example.com",
        ] {
            assert_eq!(
                check_value(&url, &json!(bad)),
                Err("Enter an https:// address.".into()),
                "{bad}"
            );
        }
    }

    fn with_condition(id: &str, option: &str, equals: Value) -> OptionSpec {
        OptionSpec {
            id: id.into(),
            visible_when: Some(VisibleWhen {
                option: option.into(),
                equals,
            }),
            ..spec(OptionKind::Text)
        }
    }

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn visible_when_is_plain_equality_on_another_value() {
        let by_string = with_condition("a", "mode", json!("manual"));
        assert!(is_visible(&by_string, &map(json!({"mode": "manual"}))));
        assert!(!is_visible(&by_string, &map(json!({"mode": "auto"}))));
        // A bool never equals the string of the same look.
        assert!(!is_visible(&by_string, &map(json!({"mode": true}))));
        let by_bool = with_condition("a", "on", json!(true));
        assert!(is_visible(&by_bool, &map(json!({"on": true}))));
        assert!(!is_visible(&by_bool, &map(json!({"on": false}))));
        assert!(!is_visible(&by_bool, &map(json!({"on": "true"}))));
        // No value for the other option (it doesn't exist): nothing is hidden.
        assert!(is_visible(&by_bool, &map(json!({}))));
        assert!(is_visible(&spec(OptionKind::Text), &map(json!({}))));
    }

    #[test]
    fn effective_values_fill_defaults_and_skip_secrets_and_buttons() {
        let specs = vec![
            OptionSpec {
                id: "flash".into(),
                default: Some(json!(true)),
                ..spec(OptionKind::Toggle)
            },
            OptionSpec {
                id: "ip".into(),
                ..spec(OptionKind::Text)
            },
            OptionSpec {
                id: "key".into(),
                ..spec(OptionKind::Secret)
            },
            OptionSpec {
                id: "go".into(),
                ..spec(OptionKind::Action)
            },
        ];
        let stored = map(json!({"ip": "1.2.3.4", "key": "s", "gone": 1}));
        assert_eq!(
            effective_values(&specs, &stored),
            map(json!({"flash": true, "ip": "1.2.3.4"}))
        );
        let hidden_by_default = with_condition("t", "flash", json!(false));
        let current = effective_values(&specs, &map(json!({})));
        assert!(!is_visible(&hidden_by_default, &current));
    }

    #[test]
    fn required_options_are_missing_until_they_have_a_value() {
        let required = |id: &str, kind| OptionSpec {
            id: id.into(),
            required: true,
            ..spec(kind)
        };
        let specs = vec![
            required("key", OptionKind::Secret),
            required("ip", OptionKind::Text),
            required("level", OptionKind::Number),
            required("on", OptionKind::Toggle),
            OptionSpec {
                default: Some(json!(40)),
                ..required("bright", OptionKind::Number)
            },
            OptionSpec {
                id: "optional".into(),
                ..spec(OptionKind::Text)
            },
        ];
        let none: Vec<String> = Vec::new();
        assert_eq!(
            missing_required(&specs, &map(json!({})), &none),
            ["key", "ip", "level", "on"]
        );
        let saved = vec!["key".to_owned()];
        let values = map(json!({"ip": "  ", "level": 3, "on": false}));
        assert_eq!(missing_required(&specs, &values, &saved), ["ip"]);
        let values = map(json!({"ip": "1.2.3.4", "level": 3, "on": false}));
        assert!(missing_required(&specs, &values, &saved).is_empty());
        // A secret saved for another option doesn't count.
        let other = vec!["other".to_owned()];
        assert_eq!(missing_required(&specs, &values, &other), ["key"]);
    }

    #[test]
    fn hidden_buttons_and_unknown_kinds_are_never_missing() {
        let specs = vec![
            OptionSpec {
                id: "hidden".into(),
                required: true,
                visible_when: Some(VisibleWhen {
                    option: "show".into(),
                    equals: json!(true),
                }),
                ..spec(OptionKind::Text)
            },
            OptionSpec {
                id: "show".into(),
                default: Some(json!(false)),
                ..spec(OptionKind::Toggle)
            },
            OptionSpec {
                id: "go".into(),
                required: true,
                ..spec(OptionKind::Action)
            },
            OptionSpec {
                id: "odd".into(),
                required: true,
                ..spec(OptionKind::Unsupported {
                    kind: "dial".into(),
                })
            },
        ];
        let none: Vec<String> = Vec::new();
        assert!(missing_required(&specs, &map(json!({})), &none).is_empty());
        assert_eq!(
            missing_required(&specs, &map(json!({"show": true})), &none),
            ["hidden"]
        );
    }

    #[test]
    fn section_defaults_leave_out_secrets_buttons_and_other_sections() {
        let specs = clean_options(vec![
            json!({"id":"a","kind":"toggle","label":"A","section":"Colors","default":true}),
            json!({"id":"b","kind":"color","label":"B","section":"Colors","default":"#FF0000"}),
            json!({"id":"c","kind":"text","label":"C","section":"Colors"}),
            json!({"id":"d","kind":"secret","label":"D","section":"Colors"}),
            json!({"id":"e","kind":"action","label":"E","section":"Colors"}),
            json!({"id":"f","kind":"number","label":"F","section":"Bulb","default":5}),
            json!({"id":"g","kind":"text","label":"G","default":"hi"}),
        ]);
        assert_eq!(
            defaults_for_section(&specs, Some("Colors")),
            map(json!({"a": true, "b": "#ff0000"}))
        );
        assert_eq!(
            defaults_for_section(&specs, Some("Bulb")),
            map(json!({"f": 5}))
        );
        assert_eq!(defaults_for_section(&specs, None), map(json!({"g": "hi"})));
        assert!(defaults_for_section(&specs, Some("Nope")).is_empty());
    }
}
