//! Pause math, urgent repeats and the seen set. Pure: the caller passes the
//! time, and nothing here fetches.

use std::collections::{HashSet, VecDeque};

use crate::model::ConversationId;

/// Time between two shows of an urgent message.
const REPEAT_EVERY: i64 = 120_000;
/// An urgent message repeats for this long after it was sent (10 repeats).
const REPEAT_FOR: i64 = 1_200_000;
/// Message ids remembered by [`Seen`].
const SEEN_CAP: usize = 512;

/// How long "Pause notifications" lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseFor {
    OneHour,
    TwoHours,
    UntilTomorrow,
}

/// When a pause started `now` ends.
pub fn pause_until(choice: PauseFor, now: i64) -> i64 {
    match choice {
        PauseFor::OneHour => now + 3_600_000,
        PauseFor::TwoHours => now + 2 * 3_600_000,
        PauseFor::UntilTomorrow => crate::time::start_of_day(now) + 32 * 3_600_000,
    }
}

/// Whether a pause that ends at `until` still holds at `now`.
pub fn is_paused(until: Option<i64>, now: i64) -> bool {
    until.is_some_and(|until| now < until)
}

/// An urgent message to show again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrgentRepeat {
    pub account: String,
    pub conversation: ConversationId,
    pub message: String,
}

#[derive(Debug)]
struct Tracked {
    repeat: UrgentRepeat,
    created: i64,
    next: i64,
}

/// Urgent messages waiting to be shown again until read.
#[derive(Debug, Default)]
pub struct UrgentRepeats {
    tracked: Vec<Tracked>,
}

impl UrgentRepeats {
    /// Start repeating a message. Duplicates and messages sent 20 minutes
    /// ago or more are ignored.
    pub fn track(
        &mut self,
        account: &str,
        conversation: &ConversationId,
        message: &str,
        created: i64,
        now: i64,
    ) {
        if self.tracked.iter().any(|t| {
            t.repeat.account == account
                && t.repeat.conversation == *conversation
                && t.repeat.message == message
        }) {
            return;
        }
        let elapsed = (now - created).max(0);
        let next = created + (elapsed / REPEAT_EVERY + 1) * REPEAT_EVERY;
        if next > created + REPEAT_FOR {
            return;
        }
        self.tracked.push(Tracked {
            repeat: UrgentRepeat {
                account: account.to_owned(),
                conversation: conversation.clone(),
                message: message.to_owned(),
            },
            created,
            next,
        });
    }

    /// The chat was opened or read up to `read_until`: stop the repeats of
    /// messages created at or before that.
    pub fn stop(&mut self, account: &str, conversation: &ConversationId, read_until: i64) {
        self.tracked.retain(|t| {
            !(t.repeat.account == account
                && t.repeat.conversation == *conversation
                && t.created <= read_until)
        });
    }

    /// Messages to show again now. Missed repeats are skipped, not replayed.
    pub fn due(&mut self, now: i64) -> Vec<UrgentRepeat> {
        let mut out = Vec::new();
        for t in &mut self.tracked {
            if t.next <= now {
                out.push(t.repeat.clone());
                while t.next <= now {
                    t.next += REPEAT_EVERY;
                }
            }
        }
        self.tracked.retain(|t| t.next <= t.created + REPEAT_FOR);
        out
    }

    /// When the next repeat is due, if any.
    pub fn next_due(&self) -> Option<i64> {
        self.tracked.iter().map(|t| t.next).min()
    }
}

/// Message ids already notified (bounded).
#[derive(Debug, Default)]
pub struct Seen {
    keys: HashSet<String>,
    order: VecDeque<String>,
}

impl Seen {
    /// True the first time a key is seen.
    pub fn first(&mut self, key: &str) -> bool {
        if self.keys.contains(key) {
            return false;
        }
        if self.order.len() >= SEEN_CAP
            && let Some(oldest) = self.order.pop_front()
        {
            self.keys.remove(&oldest);
        }
        self.keys.insert(key.to_owned());
        self.order.push_back(key.to_owned());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str) -> ConversationId {
        ConversationId::Chat(id.into())
    }

    #[test]
    fn pause_examples() {
        assert_eq!(pause_until(PauseFor::OneHour, 0), 3_600_000);
        assert_eq!(pause_until(PauseFor::TwoHours, 5), 7_200_005);
        assert!(!is_paused(Some(10), 10));
        assert!(is_paused(Some(10), 9));
        assert!(!is_paused(None, 0));
    }

    #[test]
    fn until_tomorrow_is_next_eight_local() {
        let now = 1_700_000_000_000;
        let until = pause_until(PauseFor::UntilTomorrow, now);
        assert_eq!(until, crate::time::start_of_day(now) + 32 * 3_600_000);
        assert!(until > now);
        assert!(until - now <= 32 * 3_600_000);
    }

    #[test]
    fn urgent_repeats_every_two_minutes() {
        let mut r = UrgentRepeats::default();
        r.track("a", &chat("c"), "m", 0, 0);
        assert_eq!(r.next_due(), Some(120_000));
        assert!(r.due(119_999).is_empty());
        let due = r.due(120_000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].message, "m");
        assert_eq!(due[0].account, "a");
        assert!(r.due(120_001).is_empty());
        assert_eq!(r.next_due(), Some(240_000));
    }

    #[test]
    fn urgent_stops_after_twenty_minutes() {
        let mut r = UrgentRepeats::default();
        r.track("a", &chat("c"), "m", 0, 0);
        let mut shown = 0;
        for step in 1..=20 {
            shown += r.due(step * REPEAT_EVERY).len();
        }
        assert_eq!(shown, 10);
        assert_eq!(r.next_due(), None);
        assert!(r.due(10_000_000).is_empty());
    }

    #[test]
    fn last_repeat_is_at_twenty_minutes() {
        let mut r = UrgentRepeats::default();
        r.track("a", &chat("c"), "m", 0, 0);
        assert_eq!(r.due(1_200_000).len(), 1);
        assert_eq!(r.next_due(), None);
    }

    #[test]
    fn track_ignores_duplicates_and_stale() {
        let mut r = UrgentRepeats::default();
        r.track("a", &chat("c"), "m", 0, 0);
        r.track("a", &chat("c"), "m", 0, 0);
        assert_eq!(r.due(120_000).len(), 1);
        let mut r = UrgentRepeats::default();
        r.track("a", &chat("c"), "old", 0, 1_200_000);
        r.track("a", &chat("c"), "older", 0, 5_000_000);
        assert_eq!(r.next_due(), None);
        // The same message id in another account or chat is another message.
        r.track("a", &chat("c"), "m", 0, 0);
        r.track("b", &chat("c"), "m", 0, 0);
        r.track("a", &chat("d"), "m", 0, 0);
        assert_eq!(r.due(120_000).len(), 3);
    }

    #[test]
    fn track_late_waits_for_the_next_slot() {
        let mut r = UrgentRepeats::default();
        r.track("a", &chat("c"), "m", 0, 130_000);
        assert_eq!(r.next_due(), Some(240_000));
    }

    #[test]
    fn stop_clears_read_messages_only() {
        let mut r = UrgentRepeats::default();
        r.track("a", &chat("c"), "old", 0, 0);
        r.track("a", &chat("c"), "new", 50, 50);
        r.track("a", &chat("other"), "x", 0, 0);
        r.track("b", &chat("c"), "y", 0, 0);
        r.stop("a", &chat("c"), 0);
        let due = r.due(200_000);
        let mut ids: Vec<_> = due.iter().map(|d| d.message.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["new", "x", "y"]);
        r.stop("a", &chat("c"), 100);
        r.stop("a", &chat("other"), 0);
        r.stop("b", &chat("c"), 0);
        assert_eq!(r.next_due(), None);
    }

    #[test]
    fn seen_is_first_once() {
        let mut s = Seen::default();
        assert!(s.first("a"));
        assert!(!s.first("a"));
        assert!(s.first("b"));
    }

    #[test]
    fn seen_is_bounded() {
        let mut s = Seen::default();
        for i in 0..512 {
            assert!(s.first(&i.to_string()));
        }
        assert!(!s.first("0"));
        assert!(s.first("512"));
        // "0" was the oldest and is gone; "1" is still remembered.
        assert!(!s.first("1"));
        assert!(s.first("0"));
    }
}
