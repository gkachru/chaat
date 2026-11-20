//! Presence for plugins that ask for it (`needs: presence`): the busiest
//! presence across all signed-in accounts, as availability and activity
//! only. The status message never leaves Chaat.

use crate::model::{Availability, Presence};
use crate::plugin::protocol::HostMessage;

/// Higher is busier.
fn rank(availability: Availability) -> u8 {
    match availability {
        Availability::DoNotDisturb => 6,
        Availability::Busy => 5,
        Availability::BeRightBack => 4,
        Availability::Away => 3,
        Availability::Available => 2,
        Availability::Offline => 1,
        Availability::Unknown => 0,
    }
}

fn is_call_or_meeting(presence: &Presence) -> bool {
    matches!(
        presence.activity.as_deref(),
        Some("Presenting" | "InAConferenceCall" | "InACall" | "InAMeeting")
    )
}

/// The busiest of `presences`, without its status message. A tie goes to a
/// call or meeting activity. `None` when nothing is known.
pub fn busiest(presences: &[&Presence]) -> Option<Presence> {
    let mut best: Option<&Presence> = None;
    for &presence in presences {
        if presence.availability == Availability::Unknown {
            continue;
        }
        let better = match best {
            None => true,
            Some(current) => {
                let (new, old) = (rank(presence.availability), rank(current.availability));
                new > old
                    || (new == old && is_call_or_meeting(presence) && !is_call_or_meeting(current))
            }
        };
        if better {
            best = Some(presence);
        }
    }
    best.map(|presence| Presence {
        availability: presence.availability,
        activity: presence.activity.clone(),
        status_message: None,
    })
}

fn wire_name(availability: Availability) -> &'static str {
    match availability {
        Availability::Available => "Available",
        Availability::Busy => "Busy",
        Availability::DoNotDisturb => "DoNotDisturb",
        Availability::BeRightBack => "BeRightBack",
        Availability::Away => "Away",
        Availability::Offline => "Offline",
        Availability::Unknown => "Unknown",
    }
}

/// The `presence` message for a plugin; `Unknown` when nothing is known.
pub fn message(presence: Option<&Presence>) -> HostMessage {
    match presence {
        Some(presence) => HostMessage::Presence {
            availability: wire_name(presence.availability).to_owned(),
            activity: presence.activity.clone(),
        },
        None => HostMessage::Presence {
            availability: "Unknown".to_owned(),
            activity: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(availability: Availability, activity: Option<&str>) -> Presence {
        Presence {
            availability,
            activity: activity.map(str::to_owned),
            status_message: Some("secret note".into()),
        }
    }

    #[test]
    fn busiest_ranks_dnd_busy_brb_away_available_offline() {
        use Availability::*;
        let order = [
            DoNotDisturb,
            Busy,
            BeRightBack,
            Away,
            Available,
            Offline,
            Unknown,
        ];
        for (i, high) in order.iter().enumerate() {
            for low in &order[i + 1..] {
                let (a, b) = (p(*high, None), p(*low, None));
                assert_eq!(
                    busiest(&[&b, &a]).map(|x| x.availability),
                    Some(*high),
                    "{high:?} over {low:?}"
                );
            }
        }
        let busy = p(Busy, Some("InACall"));
        assert_eq!(
            busiest(&[&p(Available, None), &busy]),
            Some(Presence {
                availability: Busy,
                activity: Some("InACall".into()),
                status_message: None,
            })
        );
    }

    #[test]
    fn a_call_activity_wins_a_tie() {
        let plain = p(Availability::Busy, None);
        for activity in ["Presenting", "InAConferenceCall", "InACall", "InAMeeting"] {
            let call = p(Availability::Busy, Some(activity));
            let first = busiest(&[&plain, &call]).unwrap();
            let second = busiest(&[&call, &plain]).unwrap();
            assert_eq!(first.activity.as_deref(), Some(activity));
            assert_eq!(second.activity.as_deref(), Some(activity));
        }
        let other = p(Availability::Busy, Some("Busy"));
        assert_eq!(busiest(&[&plain, &other]).unwrap().activity, None);
    }

    #[test]
    fn nothing_known_is_none() {
        assert_eq!(busiest(&[]), None);
        assert_eq!(busiest(&[&p(Availability::Unknown, None)]), None);
    }

    #[test]
    fn status_messages_never_leave() {
        let presence = p(Availability::Busy, Some("InACall"));
        assert_eq!(busiest(&[&presence]).unwrap().status_message, None);
        let message = message(Some(&presence));
        assert!(!message.to_line().contains("secret note"));
        assert_eq!(
            message,
            HostMessage::Presence {
                availability: "Busy".into(),
                activity: Some("InACall".into()),
            }
        );
    }

    #[test]
    fn message_uses_wire_names() {
        use Availability::*;
        for (a, name) in [
            (Available, "Available"),
            (Busy, "Busy"),
            (DoNotDisturb, "DoNotDisturb"),
            (BeRightBack, "BeRightBack"),
            (Away, "Away"),
            (Offline, "Offline"),
            (Unknown, "Unknown"),
        ] {
            let HostMessage::Presence { availability, .. } = message(Some(&p(a, None))) else {
                panic!("presence");
            };
            assert_eq!(availability, name);
        }
        assert_eq!(
            message(None),
            HostMessage::Presence {
                availability: "Unknown".into(),
                activity: None,
            }
        );
    }
}
