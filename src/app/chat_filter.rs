//! Which chats the chat list shows: the filter field and the Unread, Groups,
//! Meetings and Hidden chips.

use crate::app::App;
use crate::model::{Chat, ChatKind, ConversationId, UnifiedFilter};

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
    let rules = Rules {
        text: &app.chat_filter,
        unread_only: app.unread_only,
        groups_only: app.groups_only,
        meetings_only: app.meetings_only,
        hidden_only: app.hidden_only,
    };
    passes(
        &rules,
        chat,
        me,
        app.unread_kept.as_deref(),
        app.open.as_ref(),
    )
}

/// The filter fields `passes` reads, borrowed so no frame copies the text.
struct Rules<'a> {
    text: &'a str,
    unread_only: bool,
    groups_only: bool,
    meetings_only: bool,
    hidden_only: bool,
}

impl<'a> From<&'a UnifiedFilter> for Rules<'a> {
    fn from(f: &'a UnifiedFilter) -> Self {
        Rules {
            text: &f.text,
            unread_only: f.unread_only,
            groups_only: f.groups_only,
            meetings_only: f.meetings_only,
            hidden_only: f.hidden_only,
        }
    }
}

/// The per-chat rules of `listed`, for any filter and account state.
fn passes(
    filter: &Rules,
    chat: &Chat,
    me: &str,
    unread_kept: Option<&str>,
    open: Option<&ConversationId>,
) -> bool {
    let kept = unread_kept == Some(chat.id.as_str())
        && open == Some(&ConversationId::Chat(chat.id.clone()));
    chat.hidden == filter.hidden_only
        && matches_filter(chat, filter.text, me)
        && matches_kind(chat, filter.groups_only, filter.meetings_only)
        && (filter.hidden_only || !filter.unread_only || chat.is_unread(me) || kept)
}

/// The Unified Chats list: (account, chat index) pairs from every account
/// that pass `app.unified_filter`, newest activity first.
pub fn unified_rows(app: &App) -> Vec<(usize, usize)> {
    let rules = Rules::from(&app.unified_filter);
    let mut rows: Vec<(usize, usize)> = Vec::new();
    for (a, account) in app.accounts.iter().enumerate() {
        let me = account.me();
        for (i, chat) in account.chats.iter().enumerate() {
            if passes(
                &rules,
                chat,
                me,
                account.unread_kept.as_deref(),
                account.open.as_ref(),
            ) {
                rows.push((a, i));
            }
        }
    }
    rows.sort_by_key(|&(a, i)| std::cmp::Reverse(app.accounts[a].chats[i].activity()));
    rows
}

/// The Unified Chats list with one row per chat id: (primary account, chat
/// index in it, other accounts holding the same chat). A row shows when any
/// copy passes the filter, so it counts as unread if any copy is. The primary
/// is the active account when it holds the chat, else the first account that
/// does. Newest activity across the copies first.
pub fn unified_merged_rows(app: &App) -> Vec<(usize, usize, Vec<usize>)> {
    // (chat id, copies as (account, index), whether any copy passes).
    type Group = (String, Vec<(usize, usize)>, bool);
    let rules = Rules::from(&app.unified_filter);
    let mut groups: Vec<Group> = Vec::new();
    // Chat id to its position in `groups`, so merging is linear.
    let mut by_id: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (a, account) in app.accounts.iter().enumerate() {
        let me = account.me();
        for (i, chat) in account.chats.iter().enumerate() {
            let ok = passes(
                &rules,
                chat,
                me,
                account.unread_kept.as_deref(),
                account.open.as_ref(),
            );
            let found = (!chat.id.is_empty())
                .then(|| by_id.get(chat.id.as_str()).copied())
                .flatten();
            match found {
                Some(g) => {
                    groups[g].1.push((a, i));
                    groups[g].2 |= ok;
                }
                None => {
                    if !chat.id.is_empty() {
                        by_id.insert(chat.id.as_str(), groups.len());
                    }
                    groups.push((chat.id.clone(), vec![(a, i)], ok));
                }
            }
        }
    }
    let activity = |&(a, i): &(usize, usize)| app.accounts[a].chats[i].activity();
    let mut rows: Vec<_> = groups
        .into_iter()
        .filter(|g| g.2)
        .map(|(_, copies, _)| {
            let newest = copies.iter().map(activity).max();
            let (pa, pi) = copies
                .iter()
                .copied()
                .find(|&(a, _)| a == app.active)
                .unwrap_or(copies[0]);
            let others = copies.iter().map(|c| c.0).filter(|&a| a != pa).collect();
            (newest, (pa, pi, others))
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    rows.into_iter().map(|r| r.1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Member;

    fn shared_app() -> (App, impl Sized) {
        let (mut app, dir) = crate::app::tests::demo_app();
        let mut copy = app.accounts[0].chats[0].clone();
        copy.id = app.accounts[0].chats[0].id.clone();
        app.accounts[1].chats.push(copy);
        (app, dir)
    }

    #[test]
    fn merged_rows_dedupe_by_id() {
        let (app, _d) = shared_app();
        let id = app.accounts[0].chats[0].id.clone();
        let rows = unified_merged_rows(&app);
        let hits: Vec<_> = rows
            .iter()
            .filter(|r| app.accounts[r.0].chats[r.1].id == id)
            .collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].2.len(), 1);
        assert_eq!(rows.len(), unified_rows(&app).len() - 1);
    }

    #[test]
    fn merged_rows_scale_and_merge_at_thousands_of_chats() {
        let (mut app, _d) = crate::app::tests::demo_app();
        let base = app.accounts[0].chats[0].clone();
        for acct in 0..2 {
            app.accounts[acct].chats = (0..3000)
                .map(|n| {
                    let mut c = base.clone();
                    c.id = format!("chat-{n}");
                    c
                })
                .collect();
        }
        let rows = unified_merged_rows(&app);
        assert_eq!(rows.len(), 3000);
        assert!(rows.iter().all(|r| r.2.len() == 1));
    }

    #[test]
    fn merged_primary_prefers_active() {
        let (mut app, _d) = shared_app();
        let id = app.accounts[0].chats[0].id.clone();
        let primary = |app: &App| {
            unified_merged_rows(app)
                .into_iter()
                .find(|r| app.accounts[r.0].chats[r.1].id == id)
                .unwrap()
        };
        app.active = 1;
        let r = primary(&app);
        assert_eq!((r.0, r.2), (1, vec![0]));
        app.active = 0;
        let r = primary(&app);
        assert_eq!((r.0, r.2), (0, vec![1]));
    }

    #[test]
    fn merged_row_unread_if_any_copy_unread() {
        let (mut app, _d) = shared_app();
        let id = app.accounts[0].chats[0].id.clone();
        // Make account 0's copy read and account 1's copy unread.
        let last = app.accounts[1].chats.len() - 1;
        app.accounts[0].chats[0].marked_unread = false;
        app.accounts[0].chats[0].preview = None;
        app.accounts[1].chats[last].marked_unread = true;
        app.unified_filter.unread_only = true;
        let rows = unified_merged_rows(&app);
        assert!(rows.iter().any(|r| app.accounts[r.0].chats[r.1].id == id));
    }

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

    #[test]
    fn unified_rows_merge_and_sort() {
        let (app, _dir) = crate::app::tests::demo_app();
        assert!(app.accounts.len() >= 2);
        let rows = unified_rows(&app);
        let accounts: std::collections::BTreeSet<usize> = rows.iter().map(|r| r.0).collect();
        assert!(accounts.len() >= 2, "chats from both accounts");
        let times: Vec<_> = rows
            .iter()
            .map(|&(a, i)| app.accounts[a].chats[i].activity())
            .collect();
        assert!(times.windows(2).all(|w| w[0] >= w[1]), "newest first");
        let visible: usize = app
            .accounts
            .iter()
            .map(|a| a.chats.iter().filter(|c| !c.hidden).count())
            .sum();
        assert_eq!(rows.len(), visible);
    }

    #[test]
    fn unified_rows_unread_filter() {
        let (mut app, _dir) = crate::app::tests::demo_app();
        app.unified_filter.unread_only = true;
        let rows = unified_rows(&app);
        assert!(!rows.is_empty());
        for (a, i) in rows {
            let account = &app.accounts[a];
            assert!(account.chats[i].is_unread(account.me()));
        }
    }

    #[test]
    fn unified_rows_text_filter() {
        let (mut app, _dir) = crate::app::tests::demo_app();
        let title = app.accounts[0].chats[0].title(app.accounts[0].me());
        app.unified_filter.text = title.to_uppercase();
        let rows = unified_rows(&app);
        assert!(rows.contains(&(0, 0)));
        app.unified_filter.text = "zzzz-no-such-chat".into();
        assert!(unified_rows(&app).is_empty());
    }

    #[test]
    fn unified_rows_hidden_only_shows_only_hidden() {
        let (mut app, _dir) = crate::app::tests::demo_app();
        app.accounts[1].chats[0].hidden = true;
        assert!(!unified_rows(&app).contains(&(1, 0)));
        app.unified_filter.hidden_only = true;
        let rows = unified_rows(&app);
        assert!(rows.contains(&(1, 0)));
        assert!(rows.iter().all(|&(a, i)| app.accounts[a].chats[i].hidden));
    }
}
