//! Helpers for call alerts from an external plugin
//! (docs/calling.md#call-alerts-plugin): ring timeouts and the checks on a
//! sign-in code and page the plugin names. The plugin host
//! (`crate::plugin::host`) runs the process and `crate::plugin::event` turns
//! its lines into events; the ring card, caller matching and its buttons are
//! Chaat's own (`app::calls`).

use std::time::Duration;

/// How long a ring card stays when the plugin doesn't say: about as long as
/// Teams rings.
pub const DEFAULT_RING: Duration = Duration::from_secs(40);

/// A ring timeout from the plugin, kept between 10 seconds and 2 minutes.
pub fn ring_timeout(seconds: Option<u64>) -> Duration {
    seconds.map_or(DEFAULT_RING, |s| Duration::from_secs(s.clamp(10, 120)))
}

/// Whether a sign-in code looks like one: short, letters, digits and dashes.
pub fn is_user_code(code: &str) -> bool {
    (4..=32).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The longest sign-in page address Chaat accepts from a plugin.
const MAX_SIGN_IN_PAGE: usize = 200;

/// Whether a sign-in page the plugin names is one of Microsoft's device
/// sign-in pages, matched exactly: HTTPS, no user info, port, query or
/// fragment, host in any case, exact path. Plugin lines are untrusted; Chaat
/// offers to open only these.
pub fn is_microsoft_sign_in_page(uri: &str) -> bool {
    if uri.len() > MAX_SIGN_IN_PAGE || uri.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some(rest) = uri.strip_prefix("https://") else {
        return false;
    };
    let Some((host, path)) = rest.split_once('/') else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    matches!(
        (host.as_str(), path),
        ("microsoft.com", "devicelogin")
            | ("www.microsoft.com", "devicelogin")
            | ("login.microsoftonline.com", "common/oauth2/deviceauth")
            | ("login.microsoft.com", "device")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_short_codes_are_shown() {
        assert!(is_user_code("ABCD1234"));
        assert!(is_user_code("AB-12"));
        assert!(!is_user_code("sign in as admin@contoso.com"));
        assert!(!is_user_code(""));
        assert!(!is_user_code(&"A".repeat(40)));
    }

    #[test]
    fn only_microsoft_sign_in_pages_are_offered() {
        assert!(is_microsoft_sign_in_page(
            "https://microsoft.com/devicelogin"
        ));
        assert!(is_microsoft_sign_in_page(
            "https://login.microsoftonline.com/common/oauth2/deviceauth"
        ));
        assert!(!is_microsoft_sign_in_page(
            "http://microsoft.com/devicelogin"
        ));
        assert!(!is_microsoft_sign_in_page(
            "https://microsoft.com.evil.example/x"
        ));
        assert!(!is_microsoft_sign_in_page(
            "https://evil.example/?microsoft.com"
        ));
        assert!(!is_microsoft_sign_in_page("https://user@evil.example/"));
    }

    #[test]
    fn sign_in_pages_must_match_exactly() {
        for ok in [
            "https://microsoft.com/devicelogin",
            "https://www.microsoft.com/devicelogin",
            "https://LOGIN.MicrosoftOnline.com/common/oauth2/deviceauth",
            "https://login.microsoft.com/device",
        ] {
            assert!(is_microsoft_sign_in_page(ok), "{ok}");
        }
        for bad in [
            "https://microsoft.com/",
            "https://microsoft.com",
            "https://microsoft.com/devicelogin/",
            "https://microsoft.com/devicelogin?otc=1",
            "https://microsoft.com/devicelogin#x",
            "https://microsoft.com:443/devicelogin",
            "https://user@microsoft.com/devicelogin",
            "https://microsoft.com@evil.example/devicelogin",
            "https://microsoft.com/DeviceLogin",
            "https://microsoft.com/devicelogin ",
            "https://microsoft.com/device\tlogin",
            "https://microsoft.com/devicelogin\n",
            "https://login.microsoft.com/devicelogin",
            "https://login.microsoftonline.com/common/oauth2/authorize",
        ] {
            assert!(!is_microsoft_sign_in_page(bad), "{bad:?}");
        }
        let long = format!("https://microsoft.com/devicelogin/{}", "a".repeat(200));
        assert!(!is_microsoft_sign_in_page(&long));
    }

    #[test]
    fn consent_urls_on_microsoft_hosts_are_refused() {
        assert!(!is_microsoft_sign_in_page(
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=x&scope=Mail.Read"
        ));
        assert!(!is_microsoft_sign_in_page(
            "https://login.microsoftonline.com/common/adminconsent?client_id=x"
        ));
    }

    #[test]
    fn ring_timeouts_are_clamped_and_default() {
        assert_eq!(ring_timeout(None), DEFAULT_RING);
        assert_eq!(ring_timeout(Some(500)), Duration::from_secs(120));
        assert_eq!(ring_timeout(Some(1)), Duration::from_secs(10));
    }
}
