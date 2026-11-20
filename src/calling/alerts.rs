//! Call alerts from an external plugin (docs/calling.md#call-alerts-plugin):
//! what Chaat says to the plugin, and how its messages become backend events.
//! The plugin host (`crate::plugin::host`) runs the process; the ring card,
//! caller matching and its buttons are Chaat's own (`app::calls`).

use std::time::Duration;

use crate::backend::Event;
use crate::plugin::host::HostEvent;
use crate::plugin::protocol::{HostMessage, PluginAccount, PluginInfo, PluginMessage, plain_text};

/// The capability a call-alerts plugin must offer in `ready`.
pub const CAPABILITY: &str = "call_alerts";

/// How long a ring card stays when the plugin doesn't say: about as long as
/// Teams rings.
pub const DEFAULT_RING: Duration = Duration::from_secs(40);

/// The `hello` for a call-alerts plugin.
pub fn hello(accounts: Vec<PluginAccount>, forget: Vec<String>) -> HostMessage {
    HostMessage::Hello {
        host: "chaat".into(),
        capabilities: vec![CAPABILITY.into()],
        accounts,
        forget,
    }
}

/// A ring timeout from the plugin, kept between 10 seconds and 2 minutes.
pub fn ring_timeout(seconds: Option<u64>) -> Duration {
    seconds.map_or(DEFAULT_RING, |s| Duration::from_secs(s.clamp(10, 120)))
}

/// The backend event for something the plugin host reported, if any.
pub fn event(reported: HostEvent) -> Option<Event> {
    match reported {
        HostEvent::Stopped { error, notify } => Some(Event::CallAlertsStatus {
            running: false,
            message: notify.then(|| format!("Call alerts: the plugin stopped ({error}).")),
        }),
        HostEvent::Unsupported => Some(Event::CallAlertsStatus {
            running: false,
            message: Some("Call alerts: this plugin doesn't provide call alerts.".into()),
        }),
        HostEvent::Message(message) => message_event(message),
    }
}

fn message_event(message: PluginMessage) -> Option<Event> {
    match message {
        PluginMessage::Ready {
            name,
            description,
            docs_url,
            forgotten,
            ..
        } => Some(Event::CallAlertsReady {
            info: PluginInfo::from_ready(name, description, docs_url),
            forgotten,
        }),
        PluginMessage::IncomingCall {
            id,
            caller,
            caller_object_id,
            video,
            group,
            thread_id,
            account,
            username,
            ring_timeout_secs,
        } => Some(Event::IncomingCall(super::RingAlert {
            id,
            caller: caller
                .and_then(|name| plain_text(&name, 100))
                .unwrap_or_else(|| "Unknown caller".to_owned()),
            caller_object_id: caller_object_id.filter(|id| !id.is_empty()),
            video,
            group,
            thread_id,
            account,
            username: username.and_then(|name| plain_text(&name, 100)),
            ring_timeout: Some(ring_timeout(ring_timeout_secs)),
        })),
        PluginMessage::SignInRequired {
            method,
            verification_uri,
            user_code,
            account,
            username,
        } => {
            // Chaat can show one kind of sign-in: a code to enter on
            // Microsoft's own sign-in page.
            if method.as_deref().is_some_and(|m| m != "device_code") {
                return Some(Event::CallAlertsStatus {
                    running: true,
                    message: Some(
                        "Call alerts: the plugin asked for a sign-in Chaat can't show.".into(),
                    ),
                });
            }
            (is_microsoft_sign_in_page(&verification_uri) && is_user_code(&user_code)).then_some(
                Event::CallAlertsSignIn {
                    account,
                    username,
                    verification_uri,
                    user_code,
                },
            )
        }
        PluginMessage::SignedIn { account, .. } => Some(Event::CallAlertsSignedIn { account }),
        PluginMessage::Listening { account, .. } => Some(Event::CallAlertsListening { account }),
        PluginMessage::Error {
            message: Some(message),
            account: Some(account),
        } => Some(Event::CallAlertsError { account, message }),
        PluginMessage::Error { message, .. } => Some(Event::CallAlertsStatus {
            running: true,
            message,
        }),
        PluginMessage::Other => None,
    }
}

/// Whether a sign-in code looks like one: short, letters, digits and dashes.
pub fn is_user_code(code: &str) -> bool {
    (4..=32).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Whether a sign-in page the plugin names is an HTTPS page on Microsoft's
/// own sign-in hosts. Plugin lines are untrusted; Chaat offers to open only
/// these.
pub fn is_microsoft_sign_in_page(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        host.as_str(),
        "microsoft.com" | "www.microsoft.com" | "login.microsoftonline.com" | "login.microsoft.com"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_for(line: &str) -> Option<Event> {
        event(HostEvent::Message(PluginMessage::parse(line)?))
    }

    #[test]
    fn only_plain_short_codes_are_shown() {
        assert!(is_user_code("ABCD1234"));
        assert!(is_user_code("AB-12"));
        assert!(!is_user_code("sign in as admin@contoso.com"));
        assert!(!is_user_code(""));
        assert!(!is_user_code(&"A".repeat(40)));
        let line = r#"{"v":1,"type":"sign_in_required","verification_uri":"https://microsoft.com/devicelogin","user_code":"Call IT now"}"#;
        assert!(event_for(line).is_none());
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
    fn a_ring_carries_a_neutral_caller_and_its_timeout() {
        let line = r#"{"v":1,"type":"incoming_call","id":"c1","caller":"Ada","caller_object_id":"u-ada","video":true,"account":"o.t","ring_timeout_secs":500}"#;
        let Some(Event::IncomingCall(ring)) = event_for(line) else {
            panic!("a ring");
        };
        assert_eq!(ring.caller_object_id.as_deref(), Some("u-ada"));
        assert_eq!(ring.ring_timeout, Some(Duration::from_secs(120)));
        assert_eq!(ring.account.as_deref(), Some("o.t"));
        // Without an object id the caller stays unresolved.
        let line = r#"{"v":1,"type":"incoming_call","id":"c2"}"#;
        let Some(Event::IncomingCall(ring)) = event_for(line) else {
            panic!("a ring");
        };
        assert_eq!(ring.caller_object_id, None);
        assert_eq!(ring.caller, "Unknown caller");
        assert_eq!(ring.ring_timeout, Some(DEFAULT_RING));
    }

    #[test]
    fn sign_ins_show_only_device_codes_on_microsoft_pages() {
        let ask = |method: &str, uri: &str| {
            event_for(&format!(
                r#"{{"v":1,"type":"sign_in_required"{method},"verification_uri":"{uri}","user_code":"AB12"}}"#
            ))
        };
        assert!(matches!(
            ask("", "https://microsoft.com/devicelogin"),
            Some(Event::CallAlertsSignIn { .. })
        ));
        assert!(matches!(
            ask(
                r#","method":"device_code""#,
                "https://microsoft.com/devicelogin"
            ),
            Some(Event::CallAlertsSignIn { .. })
        ));
        assert!(ask("", "https://evil.example/").is_none());
        assert!(matches!(
            ask(
                r#","method":"browser""#,
                "https://microsoft.com/devicelogin"
            ),
            Some(Event::CallAlertsStatus {
                message: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn plugin_lines_become_events_and_unknown_ones_are_ignored() {
        assert!(matches!(
            event_for(r#"{"v":1,"type":"signed_in","account":"o.t"}"#),
            Some(Event::CallAlertsSignedIn { account: Some(a) }) if a == "o.t"
        ));
        assert!(matches!(
            event_for(r#"{"v":1,"type":"listening","account":"o.t"}"#),
            Some(Event::CallAlertsListening { .. })
        ));
        assert!(matches!(
            event_for(r#"{"v":1,"type":"error","message":"m","account":"o.t"}"#),
            Some(Event::CallAlertsError { .. })
        ));
        assert!(matches!(
            event_for(r#"{"v":1,"type":"error","message":"m"}"#),
            Some(Event::CallAlertsStatus { running: true, .. })
        ));
        assert!(matches!(
            event_for(r#"{"v":1,"type":"ready","capabilities":["call_alerts"],"name":"N"}"#),
            Some(Event::CallAlertsReady { info, .. }) if info.name.as_deref() == Some("N")
        ));
        assert!(event_for(r#"{"v":1,"type":"something_newer"}"#).is_none());
        assert!(event_for(r#"{"v":2,"type":"signed_in"}"#).is_none());
    }

    #[test]
    fn host_failures_notify_once_and_unsupported_plugins_say_so() {
        let first = event(HostEvent::Stopped {
            error: "gone".into(),
            notify: true,
        });
        assert!(matches!(
            first,
            Some(Event::CallAlertsStatus {
                running: false,
                message: Some(_)
            })
        ));
        let repeat = event(HostEvent::Stopped {
            error: "gone".into(),
            notify: false,
        });
        assert!(matches!(
            repeat,
            Some(Event::CallAlertsStatus {
                running: false,
                message: None
            })
        ));
        assert!(matches!(
            event(HostEvent::Unsupported),
            Some(Event::CallAlertsStatus {
                running: false,
                message: Some(_)
            })
        ));
    }
}
