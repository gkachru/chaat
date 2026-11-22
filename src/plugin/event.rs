//! How what a plugin host reports becomes a plugin-neutral event. Messages
//! carry no plugin name; the app adds it where it shows them. Plugin lines
//! are untrusted: text is cleaned, sign-in pages and codes are checked.

use crate::calling::RingAlert;
use crate::calling::alerts::{is_microsoft_sign_in_page, is_user_code, ring_timeout};
use crate::plugin::agent::{AgentProposal, AgentQuery, parse_proposal, parse_query};
use crate::plugin::host::HostEvent;
use crate::plugin::protocol::{
    OptionChoice, OptionSpec, PluginInfo, PluginMessage, StatusState, account_id, clean_choices,
    clean_options, is_push_channel, option_id, plain_text, push_origin,
};
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
    /// The hash of the file about to run, for a plugin that has no pin yet.
    Fingerprint {
        sha256: String,
    },
    /// The file no longer matches its pin; it was not run.
    FileChanged {
        sha256: String,
    },
    /// How an option or action is doing, in the plugin's words (plain text,
    /// at most 200 characters).
    OptionStatus {
        /// An option id (`[a-z0-9_]{1,32}`); it may name no option Chaat knows.
        id: String,
        state: StatusState,
        message: Option<String>,
    },
    /// The new choices of one `choice` option; possibly none.
    OptionsUpdate {
        id: String,
        choices: Vec<OptionChoice>,
    },
    /// A push receiver message; see `push_event`.
    Push(PushEvent),
    /// A read request from the agents plugin, for one account.
    AgentQuery {
        request: u64,
        account: String,
        query: AgentQuery,
    },
    /// A reply or reaction proposal from the agents plugin.
    AgentPropose {
        request: u64,
        account: String,
        proposal: AgentProposal,
    },
}

/// What a plugin's receiver reported, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushEvent {
    /// The receiver is reachable at this origin (`https://host[:port]`).
    Ready { url: String },
    /// A notification body on a channel (16 to 64 of `A-Za-z0-9-_`). The body
    /// is untrusted and not yet checked.
    Body { channel: String, body: String },
    /// The receiver stopped; the reason is plain text, at most 200 characters.
    Down { reason: Option<String> },
}

/// An identifier from a plugin (a call or thread id): 1 to 200 characters,
/// no whitespace, control or invisible characters. Anything else is `None`.
fn short_token(text: String) -> Option<String> {
    let ok = !text.is_empty()
        && text.chars().count() <= 200
        && !text.chars().any(|c| {
            c.is_whitespace() || c.is_control() || crate::plugin::protocol::is_invisible(c)
        });
    ok.then_some(text)
}

/// The push event for a plugin message, if it is a well-formed one.
pub fn push_event(message: PluginMessage) -> Option<PushEvent> {
    match message {
        PluginMessage::PushReady { url } => Some(PushEvent::Ready {
            url: push_origin(&url)?,
        }),
        PluginMessage::Push { channel, body } => {
            is_push_channel(&channel).then_some(PushEvent::Body { channel, body })
        }
        PluginMessage::PushDown { reason } => Some(PushEvent::Down {
            reason: reason.and_then(|text| plain_text(&text, 200)),
        }),
        _ => None,
    }
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
        HostEvent::Fingerprint { sha256 } => Some(PluginEvent::Fingerprint { sha256 }),
        HostEvent::FileChanged { sha256 } => Some(PluginEvent::FileChanged { sha256 }),
        HostEvent::Flooding => Some(PluginEvent::Status {
            running: true,
            message: Some("The plugin sent too many messages; some were ignored.".into()),
        }),
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
            // A call without a usable id is dropped.
            id: short_token(id)?,
            caller: caller
                .and_then(|name| plain_text(&name, 100))
                .unwrap_or_else(|| "Unknown caller".to_owned()),
            caller_object_id: caller_object_id.filter(|id| crate::realtime::is_user_id(id)),
            video,
            group,
            thread_id: thread_id.and_then(short_token),
            account: account.as_deref().and_then(account_id),
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
                    account: account.as_deref().and_then(account_id),
                    verification_uri,
                    user_code,
                },
            )
        }
        PluginMessage::SignedIn { account, .. } => Some(PluginEvent::SignedIn {
            account: account.as_deref().and_then(account_id),
        }),
        PluginMessage::Listening { account, .. } => Some(PluginEvent::Listening {
            account: account_id(&account)?,
        }),
        PluginMessage::Error { message, account } => {
            let message = plain_text(&message?, 200)?;
            Some(PluginEvent::Error {
                account: account.as_deref().and_then(account_id),
                message,
            })
        }
        PluginMessage::OptionStatus { id, state, message } => Some(PluginEvent::OptionStatus {
            id: option_id(&id)?,
            state,
            message: message.and_then(|text| plain_text(&text, 200)),
        }),
        PluginMessage::OptionsUpdate { id, choices } => Some(PluginEvent::OptionsUpdate {
            id: option_id(&id)?,
            choices: clean_choices(&choices)?,
        }),
        message @ (PluginMessage::PushReady { .. }
        | PluginMessage::Push { .. }
        | PluginMessage::PushDown { .. }) => push_event(message).map(PluginEvent::Push),
        PluginMessage::AgentQuery { id, account, query } => Some(PluginEvent::AgentQuery {
            request: id,
            account: account_id(&account)?,
            query: parse_query(&query)?,
        }),
        PluginMessage::AgentPropose {
            id,
            account,
            proposal,
        } => Some(PluginEvent::AgentPropose {
            request: id,
            account: account_id(&account)?,
            proposal: parse_proposal(&proposal)?,
        }),
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
    fn pinning_and_flood_reports_become_events() {
        assert_eq!(
            event(HostEvent::Fingerprint {
                sha256: "ab".into()
            }),
            Some(PluginEvent::Fingerprint {
                sha256: "ab".into()
            })
        );
        assert_eq!(
            event(HostEvent::FileChanged {
                sha256: "cd".into()
            }),
            Some(PluginEvent::FileChanged {
                sha256: "cd".into()
            })
        );
        assert_eq!(
            event(HostEvent::Flooding),
            Some(PluginEvent::Status {
                running: true,
                message: Some("The plugin sent too many messages; some were ignored.".into()),
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
    fn ring_ids_must_be_short_plain_tokens() {
        let ring = |id: &str, object: &str, thread: &str| {
            event(HostEvent::Message(parsed(json!({
                "v": 1, "type": "incoming_call", "id": id,
                "caller_object_id": object, "thread_id": thread
            }))))
        };
        let Some(PluginEvent::IncomingCall(alert)) = ring("c1", "8c1e-42ab", "19:group@thread.v2")
        else {
            panic!("ring");
        };
        assert_eq!(alert.caller_object_id.as_deref(), Some("8c1e-42ab"));
        assert_eq!(alert.thread_id.as_deref(), Some("19:group@thread.v2"));
        let long = "x".repeat(201);
        for bad in ["", "a b", "c\u{200B}1", long.as_str()] {
            assert_eq!(ring(bad, "o2", "19:t"), None, "{bad:?}");
        }
        let Some(PluginEvent::IncomingCall(alert)) = ring("c1", "o'2", long.as_str()) else {
            panic!("ring");
        };
        assert_eq!(alert.caller_object_id, None);
        assert_eq!(alert.thread_id, None);
    }

    #[test]
    fn odd_account_ids_are_dropped() {
        let error = |account: &str| {
            event(HostEvent::Message(parsed(
                json!({"v":1,"type":"error","message":"boom","account":account}),
            )))
        };
        assert!(matches!(
            error("o1.t1"),
            Some(PluginEvent::Error {
                account: Some(_),
                ..
            })
        ));
        for bad in ["not an id", "a.b.c", "x\u{202E}.y", &"a".repeat(150)] {
            assert!(matches!(
                error(bad),
                Some(PluginEvent::Error { account: None, .. })
            ));
        }
        let signed_in = parsed(json!({"v":1,"type":"signed_in","account":"bad id"}));
        assert_eq!(
            event(HostEvent::Message(signed_in)),
            Some(PluginEvent::SignedIn { account: None })
        );
        let listening = parsed(json!({"v":1,"type":"listening","account":"bad id"}));
        assert_eq!(event(HostEvent::Message(listening)), None);
        let ring = parsed(json!({"v":1,"type":"incoming_call","id":"c",
            "account":"a b","username":"Ada\u{202E}\nL"}));
        let Some(PluginEvent::IncomingCall(alert)) = event(HostEvent::Message(ring)) else {
            panic!("ring");
        };
        assert_eq!(alert.account, None);
        assert_eq!(alert.username.as_deref(), Some("Ada L"));
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
        let unclean = parsed(json!({"v":1,"type":"error","message":"boom","account":"a b"}));
        assert!(matches!(
            event(HostEvent::Message(unclean)),
            Some(PluginEvent::Error { account: None, .. })
        ));
        let listening = parsed(json!({"v":1,"type":"listening","account":"o1.t1"}));
        assert_eq!(
            event(HostEvent::Message(listening)),
            Some(PluginEvent::Listening {
                account: "o1.t1".into()
            })
        );
    }

    #[test]
    fn push_fixtures_parse_and_serialize() {
        let fixture = |name: &str| {
            std::fs::read_to_string(format!("tests/fixtures/plugin-protocol/{name}.json"))
                .expect("fixture exists")
        };
        let channel = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJ0123456";
        assert_eq!(
            push_event(PluginMessage::parse(&fixture("push_ready")).unwrap()),
            Some(PushEvent::Ready {
                url: "https://example-words.trycloudflare.com".into()
            })
        );
        let line: serde_json::Value = serde_json::from_str(&fixture("push")).unwrap();
        assert_eq!(
            push_event(PluginMessage::parse(&fixture("push")).unwrap()),
            Some(PushEvent::Body {
                channel: channel.into(),
                body: line["body"].as_str().unwrap().into()
            })
        );
        assert_eq!(
            push_event(PluginMessage::parse(&fixture("push_down")).unwrap()),
            Some(PushEvent::Down {
                reason: Some("cloudflared stopped".into())
            })
        );
        assert!(matches!(
            event(HostEvent::Message(
                PluginMessage::parse(&fixture("push")).unwrap()
            )),
            Some(PluginEvent::Push(PushEvent::Body { .. }))
        ));
    }

    #[test]
    fn agent_fixtures_become_events() {
        let fixture = |name: &str| {
            std::fs::read_to_string(format!("tests/fixtures/plugin-protocol/{name}.json"))
                .expect("fixture exists")
        };
        let message = |name: &str| PluginMessage::parse(&fixture(name)).unwrap();
        assert_eq!(
            event(HostEvent::Message(message("agent_query"))),
            Some(PluginEvent::AgentQuery {
                request: 7,
                account: "o1.t1".into(),
                query: AgentQuery::ReadMessages {
                    conversation: "chat:19:a@thread.v2".into(),
                    before: None,
                    limit: 30
                }
            })
        );
        assert_eq!(
            event(HostEvent::Message(message("agent_propose"))),
            Some(PluginEvent::AgentPropose {
                request: 8,
                account: "o1.t1".into(),
                proposal: AgentProposal::Reply {
                    conversation: "chat:19:a@thread.v2".into(),
                    text: "On it".into(),
                    reply_to: None,
                    agent: "Claude Code".into()
                }
            })
        );
    }

    #[test]
    fn bad_agent_messages_are_dropped() {
        let parse = |line: &str| PluginMessage::parse(line).and_then(message_event);
        let query = |id: &str, account: &str, kind: &str| {
            format!(
                r#"{{"v":1,"type":"agent_query","id":{id},"account":{account},"query":{{"kind":"{kind}"}}}}"#
            )
        };
        assert!(parse(&query("1", r#""o1.t1""#, "list_chats")).is_some());
        // Odd account, unknown kind, mistyped required fields.
        assert_eq!(parse(&query("1", r#""a b""#, "list_chats")), None);
        assert_eq!(parse(&query("1", r#""o1.t1""#, "drop_table")), None);
        assert_eq!(parse(&query(r#""1""#, r#""o1.t1""#, "list_chats")), None);
        assert_eq!(parse(&query("1", "5", "list_chats")), None);
        assert_eq!(
            parse(r#"{"v":1,"type":"agent_propose","id":1,"account":"o1.t1"}"#),
            None
        );
        assert_eq!(
            parse(
                r#"{"v":1,"type":"agent_propose","id":1,"account":"o1.t1","proposal":{"kind":"reply","conversation":"c","text":"\u0007"}}"#
            ),
            None
        );
    }

    #[test]
    fn push_ready_with_bad_urls_is_dropped() {
        let long = format!("https://{}.example", "a".repeat(301));
        for url in [
            "https://h.example/x?q=1",
            "https://h.example?q=1",
            "https://h.example#f",
            "https://h.example/path",
            "https://u@h.example",
            "https://h.exam\u{200b}ple",
            "http://h.example",
            long.as_str(),
        ] {
            let message = parsed(serde_json::json!({"v":1,"type":"push_ready","url":url}));
            assert_eq!(event(HostEvent::Message(message)), None, "{url:?}");
        }
    }

    #[test]
    fn push_with_bad_channel_is_dropped() {
        let long = "a".repeat(65);
        for channel in ["short", long.as_str(), "a/b/cccccccccccccccccccc", ""] {
            let message =
                parsed(serde_json::json!({"v":1,"type":"push","channel":channel,"body":"{}"}));
            assert_eq!(event(HostEvent::Message(message)), None, "{channel:?}");
        }
    }

    #[test]
    fn push_down_reason_is_cleaned() {
        let message = parsed(serde_json::json!({
            "v":1,"type":"push_down","reason":format!("a\u{1b}[31m\nb{}", "x".repeat(300))
        }));
        let Some(PluginEvent::Push(PushEvent::Down {
            reason: Some(reason),
        })) = event(HostEvent::Message(message))
        else {
            panic!("down event");
        };
        assert!(reason.chars().count() <= 200);
        assert!(!reason.chars().any(char::is_control));
        let none = parsed(serde_json::json!({"v":1,"type":"push_down","reason":"\u{1b}"}));
        assert_eq!(
            event(HostEvent::Message(none)),
            Some(PluginEvent::Push(PushEvent::Down { reason: None }))
        );
    }

    fn event_of(value: serde_json::Value) -> Option<PluginEvent> {
        event(HostEvent::Message(parsed(value)))
    }

    #[test]
    fn option_status_becomes_an_event_with_plain_text() {
        assert_eq!(
            event_of(
                json!({"v":1,"type":"option_status","id":"find","state":"ok",
                "message":" Found\n2\u{202E} bulbs "})
            ),
            Some(PluginEvent::OptionStatus {
                id: "find".into(),
                state: StatusState::Ok,
                message: Some("Found 2 bulbs".into()),
            })
        );
        for (state, expected) in [
            ("error", StatusState::Error),
            ("working", StatusState::Working),
        ] {
            let event = event_of(json!({"v":1,"type":"option_status","id":"a","state":state}));
            assert_eq!(
                event,
                Some(PluginEvent::OptionStatus {
                    id: "a".into(),
                    state: expected,
                    message: None,
                })
            );
        }
        // A message that cleans to nothing, or isn't text, is just absent.
        let blank =
            event_of(json!({"v":1,"type":"option_status","id":"a","state":"ok","message":" "}));
        assert!(matches!(
            blank,
            Some(PluginEvent::OptionStatus { message: None, .. })
        ));
        let odd = event_of(json!({"v":1,"type":"option_status","id":"a","state":"ok","message":4}));
        assert!(matches!(
            odd,
            Some(PluginEvent::OptionStatus { message: None, .. })
        ));
        let long = event_of(
            json!({"v":1,"type":"option_status","id":"a","state":"ok","message":"x".repeat(300)}),
        );
        let Some(PluginEvent::OptionStatus {
            message: Some(message),
            ..
        }) = long
        else {
            panic!("status");
        };
        assert_eq!(message.chars().count(), 200);
    }

    #[test]
    fn option_status_with_an_unknown_state_or_bad_id_is_dropped() {
        for bad in ["done", "OK", "", "busy"] {
            let line = json!({"v":1,"type":"option_status","id":"a","state":bad}).to_string();
            assert_eq!(PluginMessage::parse(&line), None, "{bad}");
        }
        let numeric = json!({"v":1,"type":"option_status","id":"a","state":3}).to_string();
        assert_eq!(PluginMessage::parse(&numeric), None);
        let no_state = json!({"v":1,"type":"option_status","id":"a"}).to_string();
        assert_eq!(PluginMessage::parse(&no_state), None);
        for id in ["Bad Id", "", &"a".repeat(33), "a-b"] {
            let status = json!({"v":1,"type":"option_status","id":id,"state":"ok"});
            assert_eq!(event_of(status), None, "{id:?}");
        }
        let update = json!({"v":1,"type":"options_update","id":"Bad Id","choices":[]});
        assert_eq!(event_of(update), None);
    }

    #[test]
    fn options_update_cleans_choices_like_ready() {
        let update = event_of(json!({"v":1,"type":"options_update","id":"bulb","choices":[
            {"value":"a","label":"Bulb\u{202E} A"},
            {"value":"b","label":"Bulb B"}
        ]}));
        assert_eq!(
            update,
            Some(PluginEvent::OptionsUpdate {
                id: "bulb".into(),
                choices: vec![
                    OptionChoice {
                        value: "a".into(),
                        label: "Bulb A".into()
                    },
                    OptionChoice {
                        value: "b".into(),
                        label: "Bulb B".into()
                    },
                ],
            })
        );
        // No bulbs found is a real answer.
        let none = event_of(json!({"v":1,"type":"options_update","id":"bulb","choices":[]}));
        assert_eq!(
            none,
            Some(PluginEvent::OptionsUpdate {
                id: "bulb".into(),
                choices: vec![]
            })
        );
        let not_a_list = event_of(json!({"v":1,"type":"options_update","id":"bulb","choices":"x"}));
        assert!(matches!(
            not_a_list,
            Some(PluginEvent::OptionsUpdate { ref choices, .. }) if choices.is_empty()
        ));
        let many: Vec<_> = (0..13)
            .map(|i| json!({"value":format!("v{i}"),"label":"L"}))
            .collect();
        assert_eq!(
            event_of(json!({"v":1,"type":"options_update","id":"bulb","choices":many})),
            None
        );
        let dup = json!([{"value":"a","label":"A"},{"value":"a","label":"B"}]);
        assert_eq!(
            event_of(json!({"v":1,"type":"options_update","id":"bulb","choices":dup})),
            None
        );
        let hidden = json!([{"value":"a\u{200B}","label":"A"}]);
        assert_eq!(
            event_of(json!({"v":1,"type":"options_update","id":"bulb","choices":hidden})),
            None
        );
    }
}
