//! Which incoming messages raise a notification, and the text they show.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// How group chats notify.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupAlerts {
    #[default]
    All,
    Mentions,
}

/// How meeting chats notify.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeetingAlerts {
    #[default]
    All,
    AfterParticipating,
    Mentions,
}

/// The notification settings, stored in `settings.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifySettings {
    /// Show the message text in a notification.
    pub preview: bool,
    pub group_chats: GroupAlerts,
    pub meeting_chats: MeetingAlerts,
    /// Stay quiet while the person's status is Do not disturb.
    pub quiet_when_dnd: bool,
    /// Stay quiet during calls and meetings.
    pub quiet_in_calls: bool,
    /// Repeat urgent messages until read.
    pub urgent_repeat: bool,
    /// Ask the taskbar for attention.
    pub flash: bool,
    /// Paused until this time (ms since the epoch).
    pub paused_until: Option<i64>,
    /// Chats muted on this device, by conversation id.
    pub muted_chats: BTreeSet<String>,
    /// "Notify when available": user id to display name.
    pub follow_available: BTreeMap<String, String>,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            preview: true,
            group_chats: GroupAlerts::All,
            meeting_chats: MeetingAlerts::All,
            quiet_when_dnd: true,
            quiet_in_calls: false,
            urgent_repeat: true,
            flash: true,
            paused_until: None,
            muted_chats: BTreeSet::new(),
            follow_available: BTreeMap::new(),
        }
    }
}

/// What kind of conversation a message is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Direct,
    Group,
    Meeting,
    Channel,
}

/// One incoming message, as the rules see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Incoming {
    pub kind: Kind,
    pub muted: bool,
    pub mentions_me: bool,
    pub urgent: bool,
    pub participated: bool,
}

/// The state of the app and the person at that moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    /// The master switch.
    pub enabled: bool,
    pub paused: bool,
    /// The person is looking at this conversation.
    pub looking: bool,
    pub dnd: bool,
    pub in_call: bool,
}

/// Whether to notify. Urgent gets through Do not disturb and in-call quiet,
/// never the master switch, a pause or the chat being looked at.
pub fn decide(settings: &NotifySettings, incoming: &Incoming, env: &Environment) -> bool {
    if !env.enabled || env.paused || env.looking {
        return false;
    }
    if incoming.urgent {
        return true;
    }
    if env.dnd && settings.quiet_when_dnd {
        return false;
    }
    if env.in_call && settings.quiet_in_calls {
        return false;
    }
    if incoming.mentions_me {
        return true;
    }
    if incoming.muted {
        return false;
    }
    match incoming.kind {
        Kind::Direct | Kind::Channel => true,
        Kind::Group => settings.group_chats == GroupAlerts::All,
        Kind::Meeting => match settings.meeting_chats {
            MeetingAlerts::All => true,
            MeetingAlerts::AfterParticipating => incoming.participated,
            MeetingAlerts::Mentions => false,
        },
    }
}

/// The notification body. Truncation happens in the OS layer.
pub fn body(preview: bool, title: &str, author: Option<&str>, text: &str, more: usize) -> String {
    if !preview {
        return match more {
            0 => "New message".to_owned(),
            n => format!("{} new messages", n + 1),
        };
    }
    let mut out = match author {
        Some(a) if a != title => format!("{a}: {text}"),
        _ => text.to_owned(),
    };
    if more > 0 {
        out.push_str(&format!(" (+{more} more)"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Environment {
        Environment {
            enabled: true,
            ..Default::default()
        }
    }

    fn group() -> Incoming {
        Incoming {
            kind: Kind::Group,
            ..Default::default()
        }
    }

    #[test]
    fn urgent_ignores_dnd_not_pause() {
        let s = NotifySettings::default();
        let dnd = Environment { dnd: true, ..env() };
        assert!(!decide(&s, &group(), &dnd));
        let urgent = Incoming {
            urgent: true,
            ..group()
        };
        assert!(decide(&s, &urgent, &dnd));
        let calls = NotifySettings {
            quiet_in_calls: true,
            ..s.clone()
        };
        assert!(decide(
            &calls,
            &urgent,
            &Environment {
                in_call: true,
                ..env()
            }
        ));
        assert!(!decide(
            &s,
            &urgent,
            &Environment {
                paused: true,
                ..env()
            }
        ));
        assert!(!decide(
            &s,
            &urgent,
            &Environment {
                enabled: false,
                ..env()
            }
        ));
        assert!(!decide(
            &s,
            &urgent,
            &Environment {
                looking: true,
                ..env()
            }
        ));
    }

    #[test]
    fn quiet_in_calls_is_opt_in() {
        let in_call = Environment {
            in_call: true,
            ..env()
        };
        assert!(decide(&NotifySettings::default(), &group(), &in_call));
        let s = NotifySettings {
            quiet_in_calls: true,
            ..Default::default()
        };
        assert!(!decide(&s, &group(), &in_call));
    }

    #[test]
    fn mention_passes_mute() {
        let s = NotifySettings {
            group_chats: GroupAlerts::Mentions,
            ..Default::default()
        };
        let muted = Incoming {
            muted: true,
            ..group()
        };
        assert!(!decide(&s, &muted, &env()));
        let mentioned = Incoming {
            mentions_me: true,
            ..muted
        };
        assert!(decide(&s, &mentioned, &env()));
        // A mention does not pass Do not disturb.
        assert!(!decide(&s, &mentioned, &Environment { dnd: true, ..env() }));
    }

    #[test]
    fn direct_ignores_mentions_only() {
        let s = NotifySettings {
            group_chats: GroupAlerts::Mentions,
            meeting_chats: MeetingAlerts::Mentions,
            ..Default::default()
        };
        assert!(decide(&s, &Incoming::default(), &env()));
        assert!(!decide(&s, &group(), &env()));
        assert!(!decide(
            &s,
            &Incoming {
                kind: Kind::Meeting,
                ..Default::default()
            },
            &env()
        ));
        let muted = Incoming {
            muted: true,
            ..Default::default()
        };
        assert!(!decide(&s, &muted, &env()));
    }

    #[test]
    fn meeting_after_participating() {
        let s = NotifySettings {
            meeting_chats: MeetingAlerts::AfterParticipating,
            ..Default::default()
        };
        let meeting = Incoming {
            kind: Kind::Meeting,
            ..Default::default()
        };
        assert!(!decide(&s, &meeting, &env()));
        assert!(decide(
            &s,
            &Incoming {
                participated: true,
                ..meeting
            },
            &env()
        ));
        let all = NotifySettings::default();
        assert!(decide(&all, &meeting, &env()));
    }

    #[test]
    fn defaults_keep_todays_behaviour() {
        let s = NotifySettings::default();
        assert!(s.preview && s.quiet_when_dnd && s.urgent_repeat && s.flash);
        assert!(!s.quiet_in_calls);
        for kind in [Kind::Direct, Kind::Group, Kind::Meeting, Kind::Channel] {
            assert!(decide(
                &s,
                &Incoming {
                    kind,
                    ..Default::default()
                },
                &env()
            ));
        }
    }

    #[test]
    fn dnd_group_example() {
        let s = NotifySettings::default();
        let dnd = Environment { dnd: true, ..env() };
        assert!(!decide(&s, &group(), &dnd));
        assert!(decide(
            &s,
            &Incoming {
                urgent: true,
                ..group()
            },
            &dnd
        ));
    }

    #[test]
    fn body_examples() {
        assert_eq!(body(true, "Ops", Some("Ada"), "hi", 2), "Ada: hi (+2 more)");
        assert_eq!(body(false, "Ops", Some("Ada"), "hi", 0), "New message");
        assert_eq!(body(false, "x", None, "hi", 2), "3 new messages");
        assert_eq!(body(true, "Ada", Some("Ada"), "hi", 0), "hi");
        assert_eq!(body(true, "Ops", None, "hi", 0), "hi");
    }
}
