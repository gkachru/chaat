use super::*;

impl Worker {
    pub(super) fn search_people(&mut self, query: String) {
        let Some(account) = &self.account else { return };
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        let latest = Arc::clone(&self.people_searches);
        let mine = next_search(&latest);
        tokio::spawn(async move {
            if !still_latest(&latest, mine).await {
                return;
            }
            match graph.search_people(&query).await {
                Ok(people) => out.send(AccountEvent::People { query, people }),
                Err(error) => log::info!("people search unavailable: {error}"),
            }
        });
    }

    /// Looks up a name once per person every ten minutes, like photos.
    pub(super) fn user_name(&mut self, user: String) {
        let Some(account) = &self.account else { return };
        let now = Instant::now();
        if self
            .names_asked
            .get(&user)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(600))
        {
            return;
        }
        self.names_asked.insert(user.clone(), now);
        let graph = Arc::clone(&account.graph);
        let out = self.out.clone();
        tokio::spawn(async move {
            match graph.user_name(&user).await {
                Ok(name) => out.send(AccountEvent::UserName { user, name }),
                Err(error) => log::debug!("name unavailable: {error}"),
            }
        });
    }

    pub(super) fn photo(&mut self, user: String) {
        let Some(account) = &self.account else { return };
        let now = Instant::now();
        if self
            .photos_asked
            .get(&user)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(600))
        {
            return;
        }
        self.photos_asked.insert(user.clone(), now);
        let graph = Arc::clone(&account.graph);
        let store = Arc::clone(&account.store);
        let out = self.out.clone();
        let downloads = Arc::clone(&self.downloads);
        tokio::spawn(async move {
            let cached = {
                let store = store.lock().unwrap_or_else(|p| p.into_inner());
                store.photo(&user, PHOTO_MAX_AGE)
            };
            if let Some(bytes) = cached {
                out.send(AccountEvent::Photo { user, bytes });
                return;
            }
            let Ok(_permit) = downloads.acquire().await else {
                return;
            };
            match graph.photo(&user).await {
                Ok(bytes) => {
                    {
                        let store = store.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = store.save_photo(&user, bytes.as_deref());
                    }
                    out.send(AccountEvent::Photo { user, bytes });
                }
                Err(error) => log::debug!("photo unavailable: {error}"),
            }
        });
    }
}

/// Whose presence the chat list shows, from the cached list: the people of
/// one-on-one chats, then the members of group chats, each most recent chat
/// first, without repeats. Meeting chats are left out: their attendee lists
/// are long and short-lived. Graph expands at most 25 members per chat in
/// the list, so larger groups are partly covered until the chat opens.
pub(super) async fn contacts_of(store: &Arc<Mutex<Store>>, me: String, tenant: String) -> People {
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || {
        let chats = store
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .chats()
            .unwrap_or_default();
        people_to_follow(&chats, &me, &tenant)
    })
    .await
    .unwrap_or_default()
}

/// Whose presence the chat list shows, in order (see [`contacts_of`]).
#[derive(Debug, Default, PartialEq)]
pub(super) struct People {
    /// Everyone: fetched when the person looks.
    pub(super) all: Vec<String>,
    /// Those in the person's own organisation, for the presence
    /// subscription. Graph refuses (400) a subscription that includes people
    /// from other organisations, though it answers presence fetches for
    /// them. Members whose organisation Graph didn't give are kept.
    pub(super) followable: Vec<String>,
}

fn people_to_follow(chats: &[crate::model::Chat], me: &str, tenant: &str) -> People {
    use crate::model::{ChatKind, Member};
    let mut people = People::default();
    let mut add = |member: &Member| {
        let user = member.user_id.as_str();
        // A deleted account stays in old chats with its name and email
        // blanked; it has no presence, and Graph refuses to follow it.
        let deleted = member.display_name.trim().is_empty() && member.email.is_none();
        if user == me || user.is_empty() || deleted || people.all.iter().any(|p| p == user) {
            return;
        }
        people.all.push(user.to_owned());
        let ours = member
            .tenant_id
            .as_deref()
            .is_none_or(|theirs| tenant.is_empty() || theirs.eq_ignore_ascii_case(tenant));
        if ours {
            people.followable.push(user.to_owned());
        }
    };
    for chat in chats {
        if let Some(peer) = chat.peer(me) {
            add(peer);
        }
    }
    for chat in chats.iter().filter(|chat| chat.kind == ChatKind::Group) {
        for member in &chat.members {
            add(member);
        }
    }
    people
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Chat, ChatKind, Member};

    /// A chat whose members are `id` (in the person's organisation, "t-1"),
    /// `id@t-2` (another organisation) or `id@` (organisation not given).
    fn chat(kind: ChatKind, members: &[&str]) -> Chat {
        Chat {
            kind,
            members: members
                .iter()
                .map(|spec| {
                    let (id, tenant) = match spec.split_once('@') {
                        Some((id, "")) => (id, None),
                        Some((id, tenant)) => (id, Some(tenant.to_owned())),
                        None => (*spec, Some("T-1".to_owned())),
                    };
                    Member {
                        user_id: id.into(),
                        display_name: id.to_uppercase(),
                        email: None,
                        tenant_id: tenant,
                    }
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn one_on_one_contacts_come_before_group_members_and_meetings_are_left_out() {
        let chats = [
            chat(ChatKind::Group, &["me", "grace", "ada"]),
            chat(ChatKind::OneOnOne, &["me", "bo"]),
            chat(ChatKind::Meeting, &["me", "visitor"]),
            chat(ChatKind::OneOnOne, &["ada", "me"]),
            chat(ChatKind::Group, &["me", "bo", "hedy"]),
        ];
        let people = people_to_follow(&chats, "me", "t-1");
        assert_eq!(people.all, ["bo", "ada", "grace", "hedy"]);
        assert_eq!(people.followable, people.all);
    }

    #[test]
    fn deleted_accounts_are_neither_fetched_nor_followed() {
        let mut gone = chat(ChatKind::Group, &["me", "ada", "gone"]);
        gone.members[2].display_name.clear();
        let people = people_to_follow(&[gone], "me", "t-1");
        assert_eq!(people.all, ["ada"]);
        assert_eq!(people.followable, ["ada"]);
    }

    #[test]
    fn people_from_other_organisations_are_fetched_but_not_followed() {
        let chats = [
            chat(ChatKind::OneOnOne, &["me", "partner@t-2"]),
            chat(ChatKind::OneOnOne, &["me", "ada"]),
            chat(ChatKind::Group, &["me", "guest@t-3", "unknown@", "grace"]),
        ];
        let people = people_to_follow(&chats, "me", "t-1");
        assert_eq!(people.all, ["partner", "ada", "guest", "unknown", "grace"]);
        assert_eq!(people.followable, ["ada", "unknown", "grace"]);
    }
}
