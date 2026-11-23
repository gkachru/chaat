//! Which chats the chat list shows: the filter field and the Unread, Groups,
//! Meetings and Hidden chips.

use crate::app::App;
use crate::model::{Chat, ChatKind, ConversationId};

/// Whether a chat matches the filter: its title or any member's name.
fn matches_filter(chat: &Chat, filter: &str, me: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return true;
    }
    chat.title(me).to_lowercase().contains(&filter)
        || chat
            .members
            .iter()
            .any(|m| m.display_name.to_lowercase().contains(&filter))
}

/// Whether a chat's kind passes the Groups and Meetings chips. With neither
/// on, every kind passes; with one or both on, only the chosen kinds do.
fn matches_kind(chat: &Chat, groups: bool, meetings: bool) -> bool {
    match chat.kind {
        _ if !groups && !meetings => true,
        ChatKind::Group => groups,
        ChatKind::Meeting => meetings,
        ChatKind::OneOnOne => false,
    }
}

/// Whether a chat shows in the list. With Unread on, a chat opened while
/// unread stays too, so it does not vanish the moment it is read. With Hidden
/// on, only hidden chats show (and Unread does not apply); otherwise none do.
pub fn listed(app: &App, chat: &Chat, me: &str) -> bool {
    let kept = app.unread_kept.as_deref() == Some(chat.id.as_str())
        && app.open.as_ref() == Some(&ConversationId::Chat(chat.id.clone()));
    chat.hidden == app.hidden_only
        && matches_filter(chat, &app.chat_filter, me)
        && matches_kind(chat, app.groups_only, app.meetings_only)
        && (app.hidden_only || !app.unread_only || chat.is_unread(me) || kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Member;

    #[test]
    fn filter_matches_titles_and_members() {
        let chat = Chat {
            topic: Some("Launch crew".into()),
            members: vec![Member {
                tenant_id: None,
                user_id: "u-ada".into(),
                display_name: "Ada Lovelace".into(),
                email: None,
            }],
            ..Default::default()
        };
        assert!(matches_filter(&chat, "", "me"));
        assert!(matches_filter(&chat, "LAUNCH", "me"));
        assert!(matches_filter(&chat, "ada", "me"));
        assert!(!matches_filter(&chat, "grace", "me"));
    }

    #[test]
    fn listed_respects_hidden_only() {
        let (backend, _rx) = crate::backend::Backend::recording();
        let mut app = App::with_backend(
            crate::paths::AppDirs::under(&std::env::temp_dir().join("chaat-test-unused")),
            crate::settings::Settings::default(),
            crate::backend::Waker::default(),
            backend,
            false,
        );
        let shown = Chat {
            id: "a".into(),
            ..Default::default()
        };
        let hidden = Chat {
            id: "b".into(),
            hidden: true,
            ..Default::default()
        };
        assert!(listed(&app, &shown, "me"));
        assert!(!listed(&app, &hidden, "me"));
        app.hidden_only = true;
        assert!(!listed(&app, &shown, "me"));
        assert!(listed(&app, &hidden, "me"));
    }

    #[test]
    fn kind_chips_show_every_kind_when_off_and_the_chosen_kinds_when_on() {
        let of = |kind| Chat {
            kind,
            ..Default::default()
        };
        let (one, group, meeting) = (
            of(ChatKind::OneOnOne),
            of(ChatKind::Group),
            of(ChatKind::Meeting),
        );
        let shown =
            |groups, meetings| [&one, &group, &meeting].map(|c| matches_kind(c, groups, meetings));
        assert_eq!(shown(false, false), [true, true, true]);
        assert_eq!(shown(true, false), [false, true, false]);
        assert_eq!(shown(false, true), [false, false, true]);
        assert_eq!(shown(true, true), [false, true, true]);
    }
}
