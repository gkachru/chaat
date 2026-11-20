//! How what a plugin host reports becomes a plugin-neutral event. Messages
//! carry no plugin name; the app adds it where it shows them. Plugin lines
//! are untrusted: text is cleaned, sign-in pages and codes are checked.

use crate::calling::RingAlert;
use crate::calling::alerts::{is_microsoft_sign_in_page, is_user_code, ring_timeout};
use crate::plugin::host::HostEvent;
use crate::plugin::protocol::{OptionSpec, PluginInfo, PluginMessage, clean_options, plain_text};
use crate::plugin::registry;

/// Something a plugin did, for the app to act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginEvent {
    Ready {
        info: PluginInfo,
        /// Known capabilities the plugin offers.
        offers: Vec<&'static str>,
        /// Known needs the plugin asked for.
        needs: Vec<&'static str>,
        options: Vec<OptionSpec>,
        forgotten: Vec<String>,
    },
    Status {
        running: bool,
        message: Option<String>,
    },
    SignIn {
        account: Option<String>,
        verification_uri: String,
        user_code: String,
    },
    SignedIn {
        account: Option<String>,
    },
    Listening {
        account: String,
    },
    Error {
        account: Option<String>,
        message: String,
    },
    IncomingCall(RingAlert),
}

/// The event for something the plugin host reported, if any.
pub fn event(reported: HostEvent) -> Option<PluginEvent> {
    match reported {
        HostEvent::Stopped { error, notify } => Some(PluginEvent::Status {
            running: false,
            message: notify.then(|| format!("The plugin stopped ({error}).")),
        }),
        HostEvent::Unsupported => Some(PluginEvent::Status {
            running: false,
            message: Some("This plugin offers nothing Chaat can use.".into()),
        }),
        HostEvent::Message(message) => message_event(message),
    }
}

fn message_event(message: PluginMessage) -> Option<PluginEvent> {
    match message {
        PluginMessage::Ready {
            capabilities,
            name,
            description,
            docs_url,
            forgotten,
            needs,
            options,
        } => Some(PluginEvent::Ready {
            info: PluginInfo::from_ready(name, description, docs_url),
            offers: registry::offered(&capabilities),
            needs: registry::needs(&needs),
            options: clean_options(options),
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
        } => Some(PluginEvent::IncomingCall(RingAlert {
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
            ..
        } => {
            // Chaat can show one kind of sign-in: a code to enter on
            // Microsoft's own sign-in page.
            if method.as_deref().is_some_and(|m| m != "device_code") {
                return Some(PluginEvent::Status {
                    running: true,
                    message: Some("The plugin asked for a sign-in Chaat can't show.".into()),
                });
            }
            (is_microsoft_sign_in_page(&verification_uri) && is_user_code(&user_code)).then_some(
                PluginEvent::SignIn {
                    account,
                    verification_uri,
                    user_code,
                },
            )
        }
        PluginMessage::SignedIn { account, .. } => Some(PluginEvent::SignedIn { account }),
        PluginMessage::Listening { account, .. } => Some(PluginEvent::Listening { account }),
        PluginMessage::Error { message, account } => {
            let message = plain_text(&message?, 200)?;
            Some(PluginEvent::Error { account, message })
        }
        PluginMessage::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parsed(value: serde_json::Value) -> PluginMessage {
        PluginMessage::parse(&value.to_string()).expect("parses")
    }

    #[test]
    fn host_outcomes_are_neutral_statuses() {
        assert_eq!(
            event(HostEvent::Unsupported),
            Some(PluginEvent::Status {
                running: false,
                message: Some("This plugin offers nothing Chaat can use.".into()),
            })
        );
        assert_eq!(
            event(HostEvent::Stopped {
                error: "exit 1".into(),
                notify: true
            }),
            Some(PluginEvent::Status {
                running: false,
                message: Some("The plugin stopped (exit 1).".into()),
            })
        );
        assert_eq!(
            event(HostEvent::Stopped {
                error: "exit 1".into(),
                notify: false
            }),
            Some(PluginEvent::Status {
                running: false,
                message: None,
            })
        );
    }

    #[test]
    fn ready_cleans_options_and_info() {
        let ready = parsed(json!({
            "v": 1, "type": "ready",
            "capabilities": ["presence_display", "x"],
            "name": " Busy\nlight ",
            "needs": ["presence", "tokens"],
            "options": [
                {"id": "ip", "kind": "text", "label": "Bulb address"},
                {"id": "Bad Id", "kind": "text", "label": "x"}
            ]
        }));
        let Some(PluginEvent::Ready {
            info,
            offers,
            needs,
            options,
            forgotten,
        }) = event(HostEvent::Message(ready))
        else {
            panic!("ready");
        };
        assert_eq!(info.name.as_deref(), Some("Status light"));
        assert_eq!(offers, [registry::PRESENCE_DISPLAY]);
        assert_eq!(needs, ["presence"]);
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].id, "ip");
        assert!(forgotten.is_empty());
    }

    #[test]
    fn sign_in_codes_and_pages_are_checked() {
        let sign_in = |method: Option<&str>, uri: &str, code: &str| {
            let mut value = json!({
                "v": 1, "type": "sign_in_required",
                "verification_uri": uri, "user_code": code, "account": "o1.t1"
            });
            if let Some(method) = method {
                value["method"] = method.into();
            }
            event(HostEvent::Message(parsed(value)))
        };
        assert_eq!(
            sign_in(None, "https://microsoft.com/devicelogin", "ABCD1234"),
            Some(PluginEvent::SignIn {
                account: Some("o1.t1".into()),
                verification_uri: "https://microsoft.com/devicelogin".into(),
                user_code: "ABCD1234".into(),
            })
        );
        assert_eq!(
            sign_in(None, "https://evil.example/login", "ABCD1234"),
            None
        );
        assert_eq!(
            sign_in(None, "https://microsoft.com/devicelogin", "no spaces"),
            None
        );
        assert!(matches!(
            sign_in(
                Some("browser"),
                "https://microsoft.com/devicelogin",
                "ABCD1234"
            ),
            Some(PluginEvent::Status { running: true, .. })
        ));
    }

    #[test]
    fn ring_alerts_are_cleaned_and_timed() {
        let ring = parsed(json!({
            "v": 1, "type": "incoming_call", "id": "c1",
            "caller": "Grace\u{202E} Hopper", "ring_timeout_secs": 1
        }));
        let Some(PluginEvent::IncomingCall(alert)) = event(HostEvent::Message(ring)) else {
            panic!("ring");
        };
        assert_eq!(alert.caller, "Grace Hopper");
        assert_eq!(alert.ring_timeout, Some(std::time::Duration::from_secs(10)));
    }

    #[test]
    fn errors_and_other_messages() {
        let with_account = parsed(json!({"v":1,"type":"error","message":"boom","account":"o1.t1"}));
        assert_eq!(
            event(HostEvent::Message(with_account)),
            Some(PluginEvent::Error {
                account: Some("o1.t1".into()),
                message: "boom".into(),
            })
        );
        let without = parsed(json!({"v":1,"type":"error","message":"boom"}));
        assert!(matches!(
            event(HostEvent::Message(without)),
            Some(PluginEvent::Error { account: None, .. })
        ));
        assert_eq!(
            event(HostEvent::Message(parsed(json!({"v":1,"type":"error"})))),
            None
        );
        assert_eq!(event(HostEvent::Message(PluginMessage::Other)), None);
        let listening = parsed(json!({"v":1,"type":"listening","account":"o1.t1"}));
        assert_eq!(
            event(HostEvent::Message(listening)),
            Some(PluginEvent::Listening {
                account: "o1.t1".into()
            })
        );
    }
}
