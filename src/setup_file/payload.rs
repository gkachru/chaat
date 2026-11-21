//! The setup file's contents and their validation.

use std::fmt;

use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::format::SetupError;

/// What a setup file carries.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPayload {
    pub format: u32,
    pub created: String,
    pub org_label: String,
    pub client_id: String,
    pub tenant: String,
    #[serde(default)]
    pub expected_tenant: Option<String>,
    #[serde(default)]
    pub relay_url: Option<String>,
    #[serde(default)]
    pub relay_key: Option<Zeroizing<String>>,
    #[serde(default)]
    pub import_by: Option<String>,
}

impl fmt::Debug for SetupPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupPayload")
            .field("format", &self.format)
            .field("relay_url", &self.relay_url.is_some())
            .field("relay_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// What the person reviews before applying; never holds the relay key.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupSummary {
    pub org_label: String,
    pub client_id: String,
    pub tenant: String,
    pub expected_tenant: Option<String>,
    pub relay_url: Option<String>,
    pub relay_key_included: bool,
    pub created: String,
    pub import_by: Option<String>,
}

/// Checks a payload as of `today` and summarizes it.
pub fn validate(p: &SetupPayload, today: Date) -> Result<SetupSummary, SetupError> {
    if p.format != 1 {
        return Err(SetupError::Invalid);
    }
    let tenant_ok = !p.tenant.is_empty()
        && p.tenant.len() <= 253
        && p.tenant
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !tenant_ok {
        return Err(SetupError::Invalid);
    }
    if !crate::settings::is_guid(&p.client_id) {
        return Err(SetupError::Invalid);
    }
    let label_len = p.org_label.chars().count();
    if !(1..=80).contains(&label_len) || p.org_label.chars().any(char::is_control) {
        return Err(SetupError::Invalid);
    }
    if let Some(tenant) = &p.expected_tenant
        && !crate::settings::is_guid(tenant)
    {
        return Err(SetupError::Invalid);
    }
    let relay_url = match &p.relay_url {
        Some(url) => Some(crate::realtime::normalize_relay(url).ok_or(SetupError::Invalid)?),
        None => None,
    };
    if let Some(key) = &p.relay_key
        && (relay_url.is_none() || !crate::realtime::valid_relay_key(key))
    {
        return Err(SetupError::Invalid);
    }
    if let Some(by) = &p.import_by {
        let date: Date = by.parse().map_err(|_| SetupError::Invalid)?;
        if date < today {
            return Err(SetupError::Expired(date.to_string()));
        }
    }
    let created: jiff::Timestamp = p.created.parse().map_err(|_| SetupError::Invalid)?;
    let created = created.to_zoned(jiff::tz::TimeZone::UTC).date().to_string();
    Ok(SetupSummary {
        org_label: p.org_label.clone(),
        client_id: p.client_id.clone(),
        tenant: p.tenant.clone(),
        expected_tenant: p.expected_tenant.clone(),
        relay_url,
        relay_key_included: p.relay_key.is_some(),
        created,
        import_by: p.import_by.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "11111111-2222-3333-4444-555555555555";

    fn payload() -> SetupPayload {
        SetupPayload {
            format: 1,
            created: "2026-11-21T10:00:00Z".into(),
            org_label: "Acme".into(),
            client_id: GUID.into(),
            tenant: "organizations".into(),
            expected_tenant: None,
            relay_url: None,
            relay_key: None,
            import_by: None,
        }
    }

    fn today() -> Date {
        Date::new(2026, 11, 21).unwrap()
    }

    #[test]
    fn valid_payload_summarizes() {
        let mut p = payload();
        p.relay_url = Some("https://relay.example.com/".into());
        p.relay_key = Some(Zeroizing::new("k".repeat(20)));
        p.import_by = Some("2026-11-21".into());
        let s = validate(&p, today()).unwrap();
        assert_eq!(s.relay_url.as_deref(), Some("https://relay.example.com"));
        assert!(s.relay_key_included);
    }

    #[test]
    fn http_relay_invalid() {
        let mut p = payload();
        p.relay_url = Some("http://relay.example.com".into());
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
    }

    #[test]
    fn key_without_url_invalid() {
        let mut p = payload();
        p.relay_key = Some(Zeroizing::new("k".repeat(20)));
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
    }

    #[test]
    fn bad_guid_invalid() {
        let mut p = payload();
        p.client_id = "nope".into();
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
        let mut p = payload();
        p.expected_tenant = Some("nope".into());
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
    }

    #[test]
    fn label_control_char_invalid() {
        let mut p = payload();
        p.org_label = "Ac
me"
        .into();
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
        p.org_label = String::new();
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
        p.org_label = "a".repeat(81);
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
        p.org_label = "a".repeat(80);
        assert!(validate(&p, today()).is_ok());
    }

    #[test]
    fn expired_import_by() {
        let mut p = payload();
        p.import_by = Some("2026-11-20".into());
        assert_eq!(
            validate(&p, today()),
            Err(SetupError::Expired("2026-11-20".into()))
        );
        p.import_by = Some("junk".into());
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
    }

    #[test]
    fn format_zero_and_bad_tenant_invalid() {
        let mut p = payload();
        p.format = 0;
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
        for t in ["", "a/b", "x?y=1", "a b"] {
            let mut p = payload();
            p.tenant = t.into();
            assert_eq!(validate(&p, today()), Err(SetupError::Invalid), "{t}");
        }
        let mut p = payload();
        p.tenant = GUID.into();
        assert!(validate(&p, today()).is_ok());
    }

    #[test]
    fn expired_text_is_canonical_date() {
        let mut p = payload();
        p.import_by = Some("2020-01-01T00:00:00[evil]".into());
        match validate(&p, today()) {
            Err(SetupError::Expired(d)) => assert_eq!(d, "2020-01-01"),
            Err(SetupError::Invalid) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn created_must_be_rfc3339() {
        let mut p = payload();
        p.created = "yesterday".into();
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
        p.created = "2026-11-21".into();
        assert_eq!(validate(&p, today()), Err(SetupError::Invalid));
        p.created = "2026-11-21T10:00:00Z".into();
        assert_eq!(validate(&p, today()).unwrap().created, "2026-11-21");
    }

    #[test]
    fn debug_redacts_key() {
        let mut p = payload();
        p.relay_key = Some(Zeroizing::new("secretsecretsecret1".into()));
        let text = format!("{p:?}");
        assert!(text.contains("relay_key: \"<redacted>\""));
        assert!(!text.contains("secretsecret"));
    }

    #[test]
    fn unknown_fields_ignored_and_camel_case() {
        let json = format!(
            r#"{{"format":1,"created":"c","orgLabel":"A","clientId":"{GUID}","tenant":"t","extra":5}}"#
        );
        let p: SetupPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p.org_label, "A");
        assert!(p.relay_key.is_none());
    }
}
