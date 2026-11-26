//! The Graph endpoints Chaat uses, each with the delegated permission it
//! needs. docs/permissions.md lists them for administrators.

use serde_json::json;

use super::types as g;
use super::{Graph, GraphError, convert, segment};
use crate::model::{
    Channel, Chat, ConversationId, Meeting, Message, PinnedMessage, Presence, Profile, SearchHit,
    Team,
};

/// Graph's page size limit for chats and messages.
pub const PAGE: usize = 50;
/// Avatars and inline images larger than this are not downloaded.
pub const MEDIA_LIMIT: usize = 8 * 1024 * 1024;

/// A message to send: its HTML, the people it mentions (`<at id="n">` in
/// the HTML), and files already uploaded and shared.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outgoing {
    pub html: String,
    pub mentions: Vec<crate::model::MentionRef>,
    pub files: Vec<super::files::SharedFile>,
    pub images: Vec<crate::clipboard::InlineImage>,
}

impl Outgoing {
    pub fn html(html: impl Into<String>) -> Self {
        Self {
            html: html.into(),
            ..Default::default()
        }
    }

    /// The `chatMessage` body Graph takes.
    pub fn body(&self) -> serde_json::Value {
        let mut content = self.html.clone();
        for file in &self.files {
            content.push_str(&format!("<attachment id=\"{}\"></attachment>", file.id));
        }
        for (index, image) in self.images.iter().enumerate() {
            content.push_str(&format!(
                "<img src=\"../hostedContents/{}/$value\" width=\"{}\" height=\"{}\">",
                index + 1,
                image.width,
                image.height
            ));
        }
        let mut body = json!({ "body": { "contentType": "html", "content": content } });
        if !self.images.is_empty() {
            use base64::Engine;
            body["hostedContents"] = self.images.iter().enumerate().map(|(index, image)| json!({
                "@microsoft.graph.temporaryId": (index + 1).to_string(),
                "contentType": "image/png",
                "contentBytes": base64::engine::general_purpose::STANDARD.encode(&image.png),
            })).collect();
        }
        if !self.mentions.is_empty() {
            body["mentions"] = self
                .mentions
                .iter()
                .enumerate()
                .map(|(id, mention)| {
                    json!({
                        "id": id,
                        "mentionText": mention.name,
                        "mentioned": { "user": {
                            "id": mention.user_id,
                            "displayName": mention.name,
                            "userIdentityType": "aadUser",
                        }},
                    })
                })
                .collect();
        }
        if !self.files.is_empty() {
            body["attachments"] = self
                .files
                .iter()
                .map(|file| {
                    json!({
                        "id": file.id,
                        "contentType": "reference",
                        "contentUrl": file.url,
                        "name": file.name,
                    })
                })
                .collect();
        }
        body
    }
}

/// A page of messages, newest first, and the link to the older ones.
#[derive(Debug, Default)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub older: Option<String>,
}

impl Graph {
    /// `GET /me` (User.Read).
    pub async fn me(&self, tenant_id: Option<String>) -> Result<Profile, GraphError> {
        let user: g::User = self
            .get("me?$select=id,displayName,mail,userPrincipalName,jobTitle")
            .await?;
        Ok(convert::profile(user, tenant_id))
    }

    /// `GET /me/chats` with members and the newest message, newest activity
    /// first (Chat.ReadWrite). Follows up to `pages` pages of 50.
    ///
    /// Graph documents both expansions but no example combines them; if it
    /// refuses the combination, the members and the previews are fetched in
    /// two listings and joined by chat id.
    pub async fn chats(&self, pages: usize) -> Result<Vec<Chat>, GraphError> {
        let combined = format!(
            "me/chats?$expand=members,lastMessagePreview&$orderby=lastMessagePreview/createdDateTime%20desc&$top={PAGE}"
        );
        match self.pages(&combined, pages).await {
            Err(GraphError::Http { status: 400, .. }) => {}
            other => {
                return other.map(|chats| {
                    chats
                        .into_iter()
                        .map(|chat| convert::chat(chat, self.my_id()))
                        .collect()
                });
            }
        }
        log::info!(
            "Graph refused the combined chat listing; listing members and previews separately"
        );
        let mut previews: Vec<g::Chat> = self
            .pages(
                &format!("me/chats?$expand=lastMessagePreview&$orderby=lastMessagePreview/createdDateTime%20desc&$top={PAGE}"),
                pages,
            )
            .await?;
        let members: Vec<g::Chat> = self
            .pages(&format!("me/chats?$expand=members&$top={PAGE}"), pages + 2)
            .await?;
        let mut members: std::collections::HashMap<String, Vec<g::ConversationMember>> = members
            .into_iter()
            .filter_map(|chat| Some((chat.id, chat.members?)))
            .collect();
        for chat in &mut previews {
            chat.members = members.remove(&chat.id);
        }
        Ok(previews
            .into_iter()
            .map(|chat| convert::chat(chat, self.my_id()))
            .collect())
    }

    /// Follows Graph's paging links for at most `max_pages` pages (at least
    /// one) and returns every item.
    async fn pages<T: serde::de::DeserializeOwned>(
        &self,
        first: &str,
        max_pages: usize,
    ) -> Result<Vec<T>, GraphError> {
        let mut url = first.to_owned();
        let mut items = Vec::new();
        for _ in 0..max_pages.max(1) {
            let page: g::Page<T> = self.get(&url).await?;
            items.extend(page.value);
            match page.next_link {
                Some(next) => url = next,
                None => break,
            }
        }
        Ok(items)
    }

    /// The person's notes chat, built from its newest message, or `None`
    /// while they have never written to themselves (Graph answers 404).
    /// Graph refuses the chat itself, so `me` names and pictures it.
    pub async fn notes_chat(&self, me: crate::model::Member) -> Result<Option<Chat>, GraphError> {
        use crate::model::{NOTES_CHAT, Preview};
        let url = format!(
            "chats/{}/messages?$top=1&$orderby=createdDateTime%20desc",
            segment(NOTES_CHAT)
        );
        let page = match absent_as_empty(self.chat_message_page(NOTES_CHAT, &url).await.map(Some))?
        {
            Some(page) => page,
            None => return Ok(None),
        };
        let preview = page.messages.first().map(|last| Preview {
            message_id: last.id.clone(),
            author_id: last.author_id.clone(),
            author: Some(last.author.clone()),
            text: last.preview_text(),
            at: last.created,
            deleted: last.deleted,
            system: last.system,
            membership: last.membership.clone(),
        });
        Ok(Some(Chat {
            id: NOTES_CHAT.to_owned(),
            topic: Some(format!("{} (You)", me.display_name)),
            updated: preview.as_ref().map_or(0, |p| p.at),
            members: vec![me],
            preview,
            ..Default::default()
        }))
    }

    /// One chat, for a chat Chaat has not listed yet (Chat.ReadWrite).
    pub async fn chat(&self, chat: &str) -> Result<Chat, GraphError> {
        let raw: g::Chat = self
            .get(&format!(
                "chats/{}?$expand=members,lastMessagePreview",
                segment(chat)
            ))
            .await?;
        Ok(convert::chat(raw, self.my_id()))
    }

    /// A chat's newest messages, or the 50 created before `before`
    /// (Chat.ReadWrite). The cursor is a timestamp, so paging back works
    /// from the cache too.
    pub async fn chat_messages(
        &self,
        chat: &str,
        before: Option<i64>,
    ) -> Result<MessagePage, GraphError> {
        let mut url = format!(
            "chats/{}/messages?$top={PAGE}&$orderby=createdDateTime%20desc",
            segment(chat)
        );
        if let Some(before) = before {
            url.push_str(&format!(
                "&$filter=createdDateTime%20lt%20{}",
                crate::time::to_iso(before)
            ));
        }
        self.chat_message_page(chat, &url).await
    }

    /// Follow Graph's opaque paging link without rebuilding its cursor.
    pub async fn chat_message_page(
        &self,
        chat: &str,
        url: &str,
    ) -> Result<MessagePage, GraphError> {
        let conversation = ConversationId::Chat(chat.to_owned());
        let page: g::Page<g::ChatMessage> = self.get(url).await?;
        Ok(MessagePage {
            messages: page
                .value
                .into_iter()
                .map(|m| convert::message(m, Some(&conversation), self.my_id(), &self.base))
                .collect(),
            older: page.next_link,
        })
    }

    /// Messages created, edited, reacted to or deleted after `since`: the
    /// date-ranged refresh the Teams API's polling rules ask for
    /// (Chat.ReadWrite).
    pub async fn chat_changes(&self, chat: &str, since: i64) -> Result<Vec<Message>, GraphError> {
        let conversation = ConversationId::Chat(chat.to_owned());
        let since = crate::time::to_iso(since);
        let url = format!(
            "chats/{}/messages?$top={PAGE}&$orderby=lastModifiedDateTime%20desc&$filter=lastModifiedDateTime%20gt%20{since}",
            segment(chat)
        );
        // A burst larger than a few pages is reloaded as a fresh page instead.
        let raw: Vec<g::ChatMessage> = self.pages(&url, 4).await?;
        Ok(raw
            .into_iter()
            .map(|m| convert::message(m, Some(&conversation), self.my_id(), &self.base))
            .collect())
    }

    /// `POST /chats/{id}/messages` (ChatMessage.Send).
    pub async fn send_chat_message(
        &self,
        chat: &str,
        message: &Outgoing,
    ) -> Result<Message, GraphError> {
        let raw: g::ChatMessage = self
            .post(
                &format!("chats/{}/messages", segment(chat)),
                &message.body(),
            )
            .await?;
        Ok(convert::message(
            raw,
            Some(&ConversationId::Chat(chat.to_owned())),
            self.my_id(),
            &self.base,
        ))
    }

    /// `POST /chats/{id}/messages/replyWithQuote` (ChatMessage.Send).
    pub async fn reply_with_quote(
        &self,
        chat: &str,
        quoted: &str,
        message: &Outgoing,
    ) -> Result<Message, GraphError> {
        let raw: g::ChatMessage = self
            .post(
                &format!("chats/{}/messages/replyWithQuote", segment(chat)),
                &json!({ "messageIds": [quoted], "replyMessage": message.body() }),
            )
            .await?;
        Ok(convert::message(
            raw,
            Some(&ConversationId::Chat(chat.to_owned())),
            self.my_id(),
            &self.base,
        ))
    }

    /// `PATCH` a message's body: chats need Chat.ReadWrite, channels
    /// ChannelMessage.ReadWrite (admin consent).
    pub async fn edit_message(
        &self,
        conversation: &ConversationId,
        message: &str,
        html: &str,
    ) -> Result<(), GraphError> {
        self.patch(
            &message_path(conversation, message),
            &json!({ "body": { "contentType": "html", "content": html } }),
        )
        .await
    }

    /// `softDelete` (Chat.ReadWrite; channels need ChannelMessage.ReadWrite).
    pub async fn delete_message(
        &self,
        me: &str,
        conversation: &ConversationId,
        message: &str,
    ) -> Result<(), GraphError> {
        let path = match conversation {
            ConversationId::Chat(chat) => format!(
                "users/{}/chats/{}/messages/{}/softDelete",
                segment(me),
                segment(chat),
                segment(message)
            ),
            ConversationId::Channel { .. } => {
                format!("{}/softDelete", message_path(conversation, message))
            }
        };
        self.post_no_content(&path, &json!({})).await
    }

    /// `setReaction` / `unsetReaction` with a Unicode emoji
    /// (ChatMessage.Send, ChannelMessage.Send).
    pub async fn react(
        &self,
        conversation: &ConversationId,
        message: &str,
        emoji: &str,
        on: bool,
    ) -> Result<(), GraphError> {
        let action = if on { "setReaction" } else { "unsetReaction" };
        self.post_no_content(
            &format!("{}/{action}", message_path(conversation, message)),
            &json!({ "reactionType": emoji }),
        )
        .await
    }

    /// `markChatReadForUser` (Chat.ReadWrite).
    pub async fn mark_chat_read(
        &self,
        chat: &str,
        me: &str,
        tenant: &str,
    ) -> Result<(), GraphError> {
        self.post_no_content(
            &format!("chats/{}/markChatReadForUser", segment(chat)),
            &json!({ "user": { "id": me, "tenantId": tenant } }),
        )
        .await
    }

    /// Hides or unhides a chat for the person (Chat.ReadWrite).
    pub async fn hide_chat(
        &self,
        chat: &str,
        me: &str,
        tenant: &str,
        hide: bool,
    ) -> Result<(), GraphError> {
        let action = if hide { "hideForUser" } else { "unhideForUser" };
        self.post_no_content(
            &format!("chats/{}/{action}", segment(chat)),
            &json!({ "user": { "id": me, "tenantId": tenant } }),
        )
        .await
    }

    /// `markChatUnreadForUser` (Chat.ReadWrite).
    pub async fn mark_chat_unread(
        &self,
        chat: &str,
        me: &str,
        tenant: &str,
    ) -> Result<(), GraphError> {
        self.post_no_content(
            &format!("chats/{}/markChatUnreadForUser", segment(chat)),
            &json!({ "user": { "id": me, "tenantId": tenant } }),
        )
        .await
    }

    /// The pinned messages of a chat (Chat.ReadWrite).
    pub async fn pinned_messages(
        &self,
        chat: &str,
        me: &str,
    ) -> Result<Vec<PinnedMessage>, GraphError> {
        let _ = me;
        // Some chats answer 404 here although they exist (seen 22 Nov 2026
        // on a chat with nothing pinned). No pin list means nothing to show,
        // not an error for the person.
        let pins: Vec<g::PinnedChatMessage> = absent_as_empty(
            self.get::<g::Page<g::PinnedChatMessage>>(&format!(
                "chats/{}/pinnedMessages?$expand=message",
                segment(chat)
            ))
            .await
            .map(|page| page.value),
        )?;
        Ok(pins
            .into_iter()
            .filter_map(convert::pinned_message)
            .collect())
    }

    /// Pins a chat message (ChatMessage.Send).
    pub async fn pin_message(&self, chat: &str, message: &str) -> Result<(), GraphError> {
        self.post_no_content(
            &format!("chats/{}/pinnedMessages", segment(chat)),
            &json!({
                "message@odata.bind": format!(
                    "https://graph.microsoft.com/v1.0/chats/{chat}/messages/{message}"
                )
            }),
        )
        .await
    }

    /// Removes a pin (Chat.ReadWrite).
    pub async fn unpin_message(&self, chat: &str, pin: &str) -> Result<(), GraphError> {
        self.delete(&format!(
            "chats/{}/pinnedMessages/{}",
            segment(chat),
            segment(pin)
        ))
        .await
    }

    /// The join link of the meeting with this meeting ID (OnlineMeetings.Read).
    pub async fn meeting_join_url_by_id(&self, id: &str) -> Result<Option<String>, GraphError> {
        let id = id.replace('\'', "''");
        let meetings: g::Page<g::OnlineMeeting> = self
            .get(&format!(
                "me/onlineMeetings?$filter=joinMeetingIdSettings/joinMeetingId%20eq%20'{}'",
                urlencoding::encode(&id)
            ))
            .await?;
        Ok(meetings
            .value
            .into_iter()
            .find_map(|meeting| meeting.join_web_url)
            .filter(|url| !url.is_empty()))
    }

    /// `GET /me/joinedTeams` (Team.ReadBasic.All). The endpoint takes no
    /// OData parameters, so nothing is selected or filtered here.
    pub async fn joined_teams(&self) -> Result<Vec<Team>, GraphError> {
        let page: g::Page<g::Team> = self.get("me/joinedTeams").await?;
        let mut teams: Vec<Team> = page.value.into_iter().map(convert::team).collect();
        teams.sort_by_key(|team| team.name.to_lowercase());
        Ok(teams)
    }

    /// `GET /teams/{id}/allChannels`, shared channels included
    /// (Channel.ReadBasic.All).
    pub async fn channels(&self, team: &str) -> Result<Vec<Channel>, GraphError> {
        let page: g::Page<g::Channel> = self
            .get(&format!(
                "teams/{}/allChannels?$select=id,displayName,description,membershipType,webUrl",
                segment(team)
            ))
            .await?;
        let mut channels: Vec<Channel> = page
            .value
            .into_iter()
            .map(|c| convert::channel(c, team))
            .collect();
        // General first, as Teams lists it, then by name.
        channels.sort_by_key(|channel| (channel.name != "General", channel.name.to_lowercase()));
        Ok(channels)
    }

    /// A channel's newest posts with their replies (ChannelMessage.Read.All,
    /// which needs admin consent).
    pub async fn channel_messages(
        &self,
        team: &str,
        channel: &str,
        older: Option<&str>,
    ) -> Result<MessagePage, GraphError> {
        let conversation = ConversationId::Channel {
            team: team.to_owned(),
            channel: channel.to_owned(),
        };
        let url = match older {
            Some(link) => link.to_owned(),
            None => format!(
                "teams/{}/channels/{}/messages?$top=25&$expand=replies",
                segment(team),
                segment(channel)
            ),
        };
        let page: g::Page<g::ChatMessage> = self.get(&url).await?;
        Ok(MessagePage {
            messages: page
                .value
                .into_iter()
                .map(|m| convert::message(m, Some(&conversation), self.my_id(), &self.base))
                .collect(),
            older: page.next_link,
        })
    }

    /// All of a post's replies, oldest first (ChannelMessage.Read.All). Pages
    /// of 50, the endpoint's largest, up to `REPLY_PAGES` of them: more than
    /// the 200 a channel page's `$expand=replies` carries.
    pub async fn channel_replies(
        &self,
        team: &str,
        channel: &str,
        root: &str,
    ) -> Result<Vec<Message>, GraphError> {
        const REPLY_PAGES: usize = 20;
        let conversation = ConversationId::Channel {
            team: team.to_owned(),
            channel: channel.to_owned(),
        };
        let raw: Vec<g::ChatMessage> = self
            .pages(
                &format!(
                    "teams/{}/channels/{}/messages/{}/replies?$top={PAGE}",
                    segment(team),
                    segment(channel),
                    segment(root)
                ),
                REPLY_PAGES,
            )
            .await?;
        let mut replies: Vec<Message> = raw
            .into_iter()
            .map(|m| convert::message(m, Some(&conversation), self.my_id(), &self.base))
            .collect();
        replies.sort_by_key(|m| m.created);
        Ok(replies)
    }

    /// A new post, or a reply to `root` (ChannelMessage.Send).
    pub async fn send_channel_message(
        &self,
        team: &str,
        channel: &str,
        root: Option<&str>,
        message: &Outgoing,
    ) -> Result<Message, GraphError> {
        let conversation = ConversationId::Channel {
            team: team.to_owned(),
            channel: channel.to_owned(),
        };
        let mut path = format!(
            "teams/{}/channels/{}/messages",
            segment(team),
            segment(channel)
        );
        if let Some(root) = root {
            path.push_str(&format!("/{}/replies", segment(root)));
        }
        let raw: g::ChatMessage = self.post(&path, &message.body()).await?;
        Ok(convert::message(
            raw,
            Some(&conversation),
            self.my_id(),
            &self.base,
        ))
    }

    /// `POST /communications/getPresencesByUserId`, up to 650 people per
    /// call (Presence.Read.All).
    pub async fn presences(&self, users: &[String]) -> Result<Vec<(String, Presence)>, GraphError> {
        let mut all = Vec::new();
        for chunk in users.chunks(650) {
            let page: g::Page<g::Presence> = self
                .post(
                    "communications/getPresencesByUserId",
                    &json!({ "ids": chunk }),
                )
                .await?;
            all.extend(page.value.into_iter().map(convert::presence));
        }
        Ok(all)
    }

    /// The calendar between two instants, in UTC (Calendars.Read).
    pub async fn calendar(&self, from: i64, to: i64) -> Result<Vec<Meeting>, GraphError> {
        let url = format!(
            "me/calendarView?startDateTime={}&endDateTime={}&$top=100&$orderby=start/dateTime&$select=id,subject,bodyPreview,start,end,isAllDay,isCancelled,location,organizer,isOnlineMeeting,onlineMeeting,webLink",
            crate::time::to_iso(from),
            crate::time::to_iso(to)
        );
        let page: g::Page<g::Event> = self
            .get_with(&url, &[("Prefer", "outlook.timezone=\"UTC\"")])
            .await?;
        Ok(page
            .value
            .into_iter()
            .filter_map(convert::meeting)
            .collect())
    }

    /// A person's small photo, or `None` without one (User.ReadBasic.All).
    pub async fn photo(&self, user: &str) -> Result<Option<Vec<u8>>, GraphError> {
        absent_as_empty(
            self.get_bytes(
                &format!("users/{}/photos/96x96/$value", segment(user)),
                super::api::MEDIA_LIMIT,
            )
            .await
            .map(Some),
        )
    }

    /// A person's display name (User.ReadBasic.All). `None` when the
    /// directory no longer has them.
    pub async fn user_name(&self, user: &str) -> Result<Option<String>, GraphError> {
        absent_as_empty(
            self.get::<g::User>(&format!("users/{}?$select=id,displayName", segment(user)))
                .await
                .map(|found| found.display_name.filter(|n| !n.is_empty())),
        )
    }

    /// Microsoft Search over chat messages (Chat.ReadWrite; channel posts
    /// also need ChannelMessage.Read.All).
    pub async fn search_messages(&self, query: &str) -> Result<Vec<SearchHit>, GraphError> {
        let response: g::SearchResponse = self
            .post(
                "search/query",
                &json!({ "requests": [{
                    "entityTypes": ["chatMessage"],
                    "query": { "queryString": query },
                    "from": 0,
                    "size": 25
                }]}),
            )
            .await?;
        let mut hits = Vec::new();
        for set in response.value {
            for container in set.hits_containers {
                for hit in container.hits {
                    if let Some(hit) = search_hit(hit) {
                        hits.push(hit);
                    }
                }
            }
        }
        Ok(hits)
    }
}

/// A Graph change-notification subscription.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: String,
    #[serde(default)]
    pub expiration_date_time: Option<String>,
    /// What it follows, as given when it was made.
    #[serde(default)]
    pub resource: Option<String>,
}

impl Graph {
    /// `GET /subscriptions`: with delegated permissions, the subscriptions
    /// this app made for the signed-in person (no extra permission beyond the
    /// ones that made them). Follows paging links, at most 10 pages.
    pub async fn subscriptions(&self) -> Result<Vec<Subscription>, GraphError> {
        self.pages("subscriptions", 10).await
    }

    /// `POST /subscriptions` for all of the person's chat messages, without
    /// resource data (Chat.Read; Chat.ReadWrite includes it).
    pub async fn subscribe_chat_messages(
        &self,
        me: &str,
        notification_url: &str,
        lifecycle_url: &str,
        client_state: &str,
        lifetime: std::time::Duration,
    ) -> Result<Subscription, GraphError> {
        let expiration = crate::time::to_iso(crate::time::now() + lifetime.as_millis() as i64);
        self.post(
            "subscriptions",
            &json!({
                "changeType": "created,updated,deleted",
                "notificationUrl": notification_url,
                "lifecycleNotificationUrl": lifecycle_url,
                "resource": format!("/users/{}/chats/getAllMessages", segment(me)),
                "expirationDateTime": expiration,
                "clientState": client_state,
                "includeResourceData": false,
            }),
        )
        .await
    }

    pub async fn renew_subscription(
        &self,
        id: &str,
        lifetime: std::time::Duration,
    ) -> Result<(), GraphError> {
        let expiration = crate::time::to_iso(crate::time::now() + lifetime.as_millis() as i64);
        self.patch(
            &format!("subscriptions/{}", segment(id)),
            &json!({ "expirationDateTime": expiration }),
        )
        .await
    }

    pub async fn reauthorize_subscription(&self, id: &str) -> Result<(), GraphError> {
        self.post_no_content(
            &format!("subscriptions/{}/reauthorize", segment(id)),
            &json!({}),
        )
        .await
    }

    /// `POST /subscriptions` for the presence of `people` (the person first,
    /// then contacts), without resource data (Presence.Read.All). One person
    /// uses `communications/presences/{id}`; more use Graph's bulk form,
    /// `communications/presences?$filter=id in (…)`, at most 650. Graph
    /// allows one presence subscription per person and app, for at most an
    /// hour. Ids must pass [`crate::realtime::is_user_id`].
    pub async fn subscribe_presence(
        &self,
        people: &[String],
        notification_url: &str,
        lifecycle_url: &str,
        client_state: &str,
        lifetime: std::time::Duration,
    ) -> Result<Subscription, GraphError> {
        let lifetime = lifetime.min(std::time::Duration::from_secs(60 * 60));
        let expiration = crate::time::to_iso(crate::time::now() + lifetime.as_millis() as i64);
        self.post(
            "subscriptions",
            &json!({
                "changeType": "updated",
                "notificationUrl": notification_url,
                "lifecycleNotificationUrl": lifecycle_url,
                "resource": presence_resource(people),
                "expirationDateTime": expiration,
                "clientState": client_state,
                "includeResourceData": false,
            }),
        )
        .await
    }

    /// The chat of the meeting with this join link:
    /// `GET /me/onlineMeetings?$filter=JoinWebUrl eq '…'` (OnlineMeetings.Read;
    /// organisers and invited attendees), then the chat itself.
    pub async fn meeting_chat(&self, join_url: &str) -> Result<Option<Chat>, GraphError> {
        let meetings: g::Page<g::OnlineMeeting> = self
            .get(&format!(
                "me/onlineMeetings?$filter=JoinWebUrl%20eq%20'{}'",
                urlencoding::encode(join_url)
            ))
            .await?;
        let Some(thread) = meetings
            .value
            .into_iter()
            .find_map(|meeting| meeting.chat_info.and_then(|info| info.thread_id))
            .filter(|thread| !thread.is_empty())
        else {
            return Ok(None);
        };
        self.chat(&thread).await.map(Some)
    }

    /// `DELETE /subscriptions/{id}`. One that is already gone (expired, or
    /// removed by Graph) counts as deleted.
    pub async fn delete_subscription(&self, id: &str) -> Result<(), GraphError> {
        absent_as_empty(self.delete(&format!("subscriptions/{}", segment(id))).await)
    }
}

/// A 404 that means "nothing there" becomes the empty value; every other
/// error stays an error.
fn absent_as_empty<T: Default>(result: Result<T, GraphError>) -> Result<T, GraphError> {
    match result {
        Err(GraphError::NotFound) => Ok(T::default()),
        other => other,
    }
}

/// The subscription resource for these people's presence.
fn presence_resource(people: &[String]) -> String {
    debug_assert!(people.iter().all(|id| crate::realtime::is_user_id(id)));
    match people {
        [one] => format!("communications/presences/{}", segment(one)),
        many => {
            let ids: Vec<String> = many.iter().map(|id| format!("'{id}'")).collect();
            format!("communications/presences?$filter=id in ({})", ids.join(","))
        }
    }
}

fn message_path(conversation: &ConversationId, message: &str) -> String {
    match conversation {
        ConversationId::Chat(chat) => {
            format!("chats/{}/messages/{}", segment(chat), segment(message))
        }
        ConversationId::Channel { team, channel } => format!(
            "teams/{}/channels/{}/messages/{}",
            segment(team),
            segment(channel),
            segment(message)
        ),
    }
}

fn search_hit(hit: g::SearchHit) -> Option<SearchHit> {
    let resource = hit.resource?;
    let chat = resource.get("chatId").and_then(|v| v.as_str());
    let team = resource
        .pointer("/channelIdentity/teamId")
        .and_then(|v| v.as_str());
    let channel = resource
        .pointer("/channelIdentity/channelId")
        .and_then(|v| v.as_str());
    let conversation = match (chat, team, channel) {
        (Some(chat), _, _) => ConversationId::Chat(chat.to_owned()),
        (None, Some(team), Some(channel)) => ConversationId::Channel {
            team: team.to_owned(),
            channel: channel.to_owned(),
        },
        _ => return None,
    };
    Some(SearchHit {
        conversation,
        message_id: resource.get("id").and_then(|v| v.as_str())?.to_owned(),
        author: resource
            .pointer("/from/emailAddress/name")
            .or_else(|| resource.pointer("/from/user/displayName"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
        snippet: crate::html::parse(hit.summary.as_deref().unwrap_or_default()).summary(200),
        at: resource
            .get("createdDateTime")
            .and_then(|v| v.as_str())
            .and_then(crate::time::parse_iso)
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn presence_subscriptions_use_the_bulk_form_for_more_than_one_person() {
        assert_eq!(
            presence_resource(&["u-me".into()]),
            "communications/presences/u-me"
        );
        assert_eq!(
            presence_resource(&["u-me".into(), "u-ada".into(), "u-bo".into()]),
            "communications/presences?$filter=id in ('u-me','u-ada','u-bo')"
        );
    }

    async fn graph(server: &MockServer) -> Graph {
        Graph::with_base(
            reqwest::Client::new(),
            testing::session("at", &server.uri()).await,
            &server.uri(),
        )
    }

    #[tokio::test]
    async fn channel_replies_follow_next_links_oldest_first() {
        let server = MockServer::start().await;
        let replies = "/teams/t/channels/c/messages/root/replies";
        Mock::given(method("GET"))
            .and(path(replies))
            .and(query_param("$top", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{"id": "r3", "createdDateTime": "2026-10-10T09:03:00Z"}],
                "@odata.nextLink": format!("{}{replies}?$skiptoken=two", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(replies))
            .and(query_param("$skiptoken", "two"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [
                    {"id": "r2", "createdDateTime": "2026-10-10T09:02:00Z"},
                    {"id": "r1", "createdDateTime": "2026-10-10T09:01:00Z"}
                ]
            })))
            .mount(&server)
            .await;
        let got = graph(&server)
            .await
            .channel_replies("t", "c", "root")
            .await
            .unwrap();
        let ids: Vec<&str> = got.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["r1", "r2", "r3"]);
    }

    #[tokio::test]
    async fn user_names_come_from_the_directory_and_gone_users_have_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/u-grace"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "u-grace", "displayName": "Grace"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users/u-gone"))
            .respond_with(ResponseTemplate::new(404).set_body_json(
                serde_json::json!({"error": {"code": "Request_ResourceNotFound", "message": "x"}}),
            ))
            .mount(&server)
            .await;
        let graph = graph(&server).await;
        assert_eq!(
            graph.user_name("u-grace").await.unwrap().as_deref(),
            Some("Grace")
        );
        assert_eq!(graph.user_name("u-gone").await.unwrap(), None);
    }

    #[tokio::test]
    async fn deleting_a_subscription_that_is_already_gone_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/subscriptions/gone"))
            .respond_with(ResponseTemplate::new(404).set_body_json(
                serde_json::json!({"error": {"code": "ResourceNotFound", "message": "x"}}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/subscriptions/denied"))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(
                    serde_json::json!({"error": {"code": "Forbidden", "message": "x"}}),
                ),
            )
            .mount(&server)
            .await;
        let graph = graph(&server).await;
        assert!(graph.delete_subscription("gone").await.is_ok());
        assert!(graph.delete_subscription("denied").await.is_err());
    }

    #[tokio::test]
    async fn hosted_content_uses_graph_base() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/chats/c1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{"id": "m1", "createdDateTime": "2026-11-18T07:00:00Z",
                    "body": {"contentType": "html", "content": "<img src=\"../hostedContents/h1/$value\">"}}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/chats/c1/messages/m1/hostedContents/h1/$value"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8, 2, 3]))
            .mount(&server)
            .await;
        let graph = graph(&server).await;
        let page = graph
            .chat_message_page("c1", "chats/c1/messages")
            .await
            .unwrap();
        let src = page.messages[0].body.images().next().unwrap().to_owned();
        assert!(src.starts_with(&server.uri()), "{src}");
        assert!(graph.is_graph_url(&src));
        assert_eq!(graph.get_bytes(&src, 1024).await.unwrap(), vec![1u8, 2, 3]);
    }

    #[tokio::test]
    async fn hosted_content_cached_absolute_url_still_loads() {
        // Cached messages hold the production URL; with the production base
        // it is still on the Graph origin and still fetched.
        let old = "https://graph.microsoft.com/v1.0/chats/c1/messages/m1/hostedContents/h1/$value";
        let graph = Graph::with_base(
            reqwest::Client::new(),
            testing::session("at", "http://unused.invalid").await,
            crate::graph::GRAPH_BASE,
        );
        assert!(graph.is_graph_url(old));
        assert_eq!(graph.url(old).unwrap(), old);
        let raw: g::ChatMessage = serde_json::from_value(json!({
            "id": "m1",
            "body": {"contentType": "html", "content": "<img src=\"../hostedContents/h1/$value\">"}
        }))
        .unwrap();
        let message = convert::message(
            raw,
            Some(&ConversationId::Chat("c1".into())),
            "u-me",
            crate::graph::GRAPH_BASE,
        );
        assert_eq!(message.body.images().next(), Some(old));
    }

    #[tokio::test]
    async fn pages_stops_at_max() {
        let server = MockServer::start().await;
        for (n, next) in [("p1", Some("p2")), ("p2", Some("p3")), ("p3", None)] {
            let mut body = json!({"value": [{"id": n}]});
            if let Some(next) = next {
                body["@odata.nextLink"] =
                    json!(format!("{}/items?$skiptoken={next}", server.uri()));
            }
            let mock = Mock::given(method("GET")).and(path("/items"));
            let mock = if n == "p1" {
                mock.and(query_param("start", "1"))
            } else {
                mock.and(query_param("$skiptoken", n))
            };
            mock.respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
        }
        let items: Vec<serde_json::Value> = graph(&server)
            .await
            .pages("items?start=1", 2)
            .await
            .unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn pages_zero_means_one() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{"id": "a"}],
                "@odata.nextLink": format!("{}/items?$skiptoken=x", server.uri())
            })))
            .mount(&server)
            .await;
        let items: Vec<serde_json::Value> = graph(&server).await.pages("items", 0).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[test]
    fn absent_as_empty_keeps_other_errors() {
        assert_eq!(
            absent_as_empty::<Vec<u8>>(Err(GraphError::NotFound)).unwrap(),
            Vec::<u8>::new()
        );
        assert_eq!(absent_as_empty(Ok(vec![1u8])).unwrap(), vec![1u8]);
        assert!(
            absent_as_empty::<Vec<u8>>(Err(GraphError::Forbidden { code: "x".into() })).is_err()
        );
    }

    #[tokio::test]
    async fn chats_follow_next_links() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/chats"))
            .and(query_param("$top", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{"id": "19:a@thread.v2", "chatType": "oneOnOne"}],
                "@odata.nextLink": format!("{}/me/chats?$skiptoken=page2", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/me/chats"))
            .and(query_param("$skiptoken", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{"id": "19:b@thread.v2", "chatType": "group", "topic": "Launch"}]
            })))
            .mount(&server)
            .await;
        let chats = graph(&server).await.chats(3).await.unwrap();
        assert_eq!(chats.len(), 2);
        assert_eq!(chats[1].topic.as_deref(), Some("Launch"));
    }

    #[tokio::test]
    async fn the_notes_chat_comes_from_its_newest_message() {
        let me = || crate::model::Member {
            tenant_id: None,
            user_id: "u-me".into(),
            display_name: "Me".into(),
            email: None,
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/chats/48:notes/messages"))
            .and(query_param("$top", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{"id": "m1", "createdDateTime": "2026-11-18T07:00:00Z",
                    "body": {"contentType": "text", "content": "buy milk"},
                    "from": {"user": {"id": "u-me", "displayName": "Me"}}}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let notes = graph(&server)
            .await
            .notes_chat(me())
            .await
            .unwrap()
            .unwrap();
        assert!(notes.is_notes());
        assert_eq!(notes.title("u-me"), "Me (You)");
        assert_eq!(notes.face("u-me").map(|m| m.user_id.as_str()), Some("u-me"));
        assert_eq!(notes.peer("u-me"), None, "no calling yourself");
        assert!(!notes.is_unread("u-me"));
        let preview = notes.preview.as_ref().unwrap();
        assert_eq!(preview.text, "buy milk");
        assert!(preview.at > 0);

        // Someone who never wrote to themselves has no notes chat.
        let empty = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/chats/48:notes/messages"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": {"code": "NotFound", "message": "not found"}
            })))
            .mount(&empty)
            .await;
        assert_eq!(graph(&empty).await.notes_chat(me()).await.unwrap(), None);
    }

    #[test]
    fn notes_sort_with_the_other_chats() {
        let chat = |id: &str, at: i64| Chat {
            id: id.into(),
            updated: at,
            ..Default::default()
        };
        let mut chats = [
            chat("19:a", 30),
            chat(crate::model::NOTES_CHAT, 10),
            chat("19:b", 20),
        ];
        chats.sort_by_key(Chat::list_order);
        let ids: Vec<&str> = chats.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["19:a", "19:b", "48:notes"]);
    }

    #[tokio::test]
    async fn chats_fall_back_to_two_listings() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/chats"))
            .and(query_param("$expand", "members,lastMessagePreview"))
            .respond_with(
                ResponseTemplate::new(400).set_body_json(json!({"error": {"code": "BadRequest"}})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/me/chats"))
            .and(query_param("$expand", "lastMessagePreview"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{
                "id": "19:a@unq.gbl.spaces", "chatType": "oneOnOne",
                "lastMessagePreview": {"id": "1", "createdDateTime": "2026-10-11T10:00:00Z",
                    "body": {"contentType": "text", "content": "hi"}, "from": {"user": {"id": "u-ada", "displayName": "Ada"}}}
            }]})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/me/chats"))
            .and(query_param("$expand", "members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{
                "id": "19:a@unq.gbl.spaces", "chatType": "oneOnOne",
                "members": [{"userId": "u-ada", "displayName": "Ada"}, {"userId": "u-me", "displayName": "Me"}]
            }]})))
            .mount(&server)
            .await;
        let chats = graph(&server).await.chats(1).await.unwrap();
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].title("u-me"), "Ada");
        assert_eq!(chats[0].preview.as_ref().unwrap().text, "hi");
    }

    #[tokio::test]
    async fn sending_posts_html_and_returns_the_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chats/19:a@thread.v2/messages"))
            .and(body_json(
                json!({"body": {"contentType": "html", "content": "<p>hi</p>"}}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "1702", "messageType": "message", "createdDateTime": "2026-10-11T10:00:00Z",
                "from": {"user": {"id": "u-me", "displayName": "Me"}},
                "body": {"contentType": "html", "content": "<p>hi</p>"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let message = graph(&server)
            .await
            .send_chat_message("19:a@thread.v2", &Outgoing::html("<p>hi</p>"))
            .await
            .unwrap();
        assert_eq!(message.id, "1702");
        assert_eq!(message.body.to_plain(), "hi");
    }

    #[test]
    fn outgoing_messages_carry_mentions_and_files() {
        let message = Outgoing {
            html: "<p><at id=\"0\">Ada</at> see this</p>".into(),
            mentions: vec![crate::model::MentionRef {
                user_id: "u-ada".into(),
                name: "Ada".into(),
            }],
            images: Vec::new(),
            files: vec![crate::graph::files::SharedFile {
                id: "153fa47d-18c9-4179-be08-9879815a9f90".into(),
                name: "plan.pdf".into(),
                url: "https://contoso-my.sharepoint.com/personal/me/Documents/plan.pdf".into(),
            }],
        };
        assert_eq!(
            message.body(),
            json!({
                "body": {"contentType": "html", "content": "<p><at id=\"0\">Ada</at> see this</p><attachment id=\"153fa47d-18c9-4179-be08-9879815a9f90\"></attachment>"},
                "mentions": [{"id": 0, "mentionText": "Ada", "mentioned": {"user": {"id": "u-ada", "displayName": "Ada", "userIdentityType": "aadUser"}}}],
                "attachments": [{"id": "153fa47d-18c9-4179-be08-9879815a9f90", "contentType": "reference",
                    "contentUrl": "https://contoso-my.sharepoint.com/personal/me/Documents/plan.pdf", "name": "plan.pdf"}]
            })
        );
        assert_eq!(
            Outgoing::html("<p>x</p>").body(),
            json!({"body": {"contentType": "html", "content": "<p>x</p>"}})
        );
    }

    #[tokio::test]
    async fn pasted_images_are_sent_as_hosted_content_without_onedrive_permissions() {
        use base64::Engine;
        let server = MockServer::start().await;
        let image = crate::clipboard::encode_rgba(1, 1, &[255, 0, 0, 255]).unwrap();
        let expected = json!({
            "body": {"contentType": "html", "content": "<img src=\"../hostedContents/1/$value\" width=\"1\" height=\"1\">"},
            "hostedContents": [{"@microsoft.graph.temporaryId": "1", "contentType": "image/png", "contentBytes": base64::engine::general_purpose::STANDARD.encode(&image.png)}]
        });
        Mock::given(method("POST"))
            .and(path("/chats/chat/messages"))
            .and(body_json(expected))
            .respond_with(ResponseTemplate::new(201).set_body_json(
                json!({"id":"image-message", "body":{"contentType":"text","content":"image"}}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let outgoing = Outgoing {
            images: vec![image],
            ..Default::default()
        };
        graph(&server)
            .await
            .send_chat_message("chat", &outgoing)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn opaque_history_links_keep_messages_with_identical_timestamps() {
        let server = MockServer::start().await;
        let link = format!("{}/next-page?$skiptoken=opaque", server.uri());
        Mock::given(method("GET")).and(path("/chats/chat/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"@odata.nextLink":link, "value":[{"id":"first", "createdDateTime":"2026-10-10T09:00:00Z"}]})))
            .expect(1).mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/next-page"))
            .and(query_param("$skiptoken", "opaque"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"value":[{"id":"second", "createdDateTime":"2026-10-10T09:00:00Z"}]}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let graph = graph(&server).await;
        let first = graph.chat_messages("chat", None).await.unwrap();
        let second = graph
            .chat_message_page("chat", first.older.as_deref().unwrap())
            .await
            .unwrap();
        assert_eq!(first.messages[0].created, second.messages[0].created);
        assert_eq!(second.messages[0].id, "second");
        assert!(second.older.is_none());
    }

    #[tokio::test]
    async fn chat_messages_carry_recordings() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/chats/chat/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{
                "id": "1", "createdDateTime": "2026-10-10T09:00:00Z",
                "messageType": "systemEventMessage",
                "body": {"contentType": "html", "content": "<systemEventMessage/>"},
                "eventDetail": {
                    "@odata.type": "#microsoft.graph.callRecordingEventMessageDetail",
                    "callRecordingUrl": "https://c-my.sharepoint.com/r.mp4",
                    "callRecordingDisplayName": "Sync",
                    "callRecordingDuration": "PT58M",
                    "callRecordingStatus": "success"
                }
            }]})))
            .expect(1)
            .mount(&server)
            .await;
        let page = graph(&server)
            .await
            .chat_messages("chat", None)
            .await
            .unwrap();
        assert!(page.messages[0].recording.is_some());
    }

    #[tokio::test]
    async fn quoting_uses_reply_with_quote() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chats/19:a@thread.v2/messages/replyWithQuote"))
            .and(body_json(json!({"messageIds": ["1690"], "replyMessage": {"body": {"contentType": "html", "content": "<p>yes</p>"}}})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "1703", "messageType": "message"})))
            .expect(1)
            .mount(&server)
            .await;
        graph(&server)
            .await
            .reply_with_quote("19:a@thread.v2", "1690", &Outgoing::html("<p>yes</p>"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn reactions_edits_deletes_and_read_marks_hit_their_endpoints() {
        let server = MockServer::start().await;
        for (verb, route) in [
            ("POST", "/chats/19:a@thread.v2/messages/1/setReaction"),
            ("POST", "/chats/19:a@thread.v2/messages/1/unsetReaction"),
            ("PATCH", "/chats/19:a@thread.v2/messages/1"),
            (
                "POST",
                "/users/u-me/chats/19:a@thread.v2/messages/1/softDelete",
            ),
            ("POST", "/chats/19:a@thread.v2/markChatReadForUser"),
        ] {
            Mock::given(method(verb))
                .and(path(route))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&server)
                .await;
        }
        let graph = graph(&server).await;
        let chat = ConversationId::Chat("19:a@thread.v2".into());
        graph.react(&chat, "1", "👍", true).await.unwrap();
        graph.react(&chat, "1", "👍", false).await.unwrap();
        graph
            .edit_message(&chat, "1", "<p>fixed</p>")
            .await
            .unwrap();
        graph.delete_message("u-me", &chat, "1").await.unwrap();
        graph
            .mark_chat_read("19:a@thread.v2", "u-me", "t1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn changes_are_date_ranged() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/chats/19:a@thread.v2/messages"))
            .and(query_param(
                "$filter",
                "lastModifiedDateTime gt 2026-10-11T10:00:00.000Z",
            ))
            .and(query_param("$orderby", "lastModifiedDateTime desc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .expect(1)
            .mount(&server)
            .await;
        let since = crate::time::parse_iso("2026-10-11T10:00:00Z").unwrap();
        graph(&server)
            .await
            .chat_changes("19:a@thread.v2", since)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn missing_photos_are_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/u-x/photos/96x96/$value"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(json!({"error": {"code": "ImageNotFound"}})),
            )
            .mount(&server)
            .await;
        assert_eq!(graph(&server).await.photo("u-x").await.unwrap(), None);
    }

    #[tokio::test]
    async fn search_hits_name_their_conversation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{"hitsContainers": [{"hits": [{
                    "hitId": "h1", "summary": "the <c0>launch</c0> plan",
                    "resource": {"@odata.type": "#microsoft.graph.chatMessage", "id": "1704", "chatId": "19:a@thread.v2",
                        "createdDateTime": "2026-10-11T10:00:00Z", "from": {"emailAddress": {"name": "Ada"}}}
                }]}]}]
            })))
            .mount(&server)
            .await;
        let hits = graph(&server)
            .await
            .search_messages("launch")
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippet, "the launch plan");
        assert_eq!(hits[0].author, "Ada");
    }

    async fn expect_post(server: &MockServer, route: &str, body: serde_json::Value, status: u16) {
        Mock::given(method("POST"))
            .and(path(route))
            .and(body_json(body))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn hide_posts_user_body() {
        let server = MockServer::start().await;
        expect_post(
            &server,
            "/chats/19:a@thread.v2/hideForUser",
            json!({"user": {"id": "u-me", "tenantId": "t1"}}),
            204,
        )
        .await;
        graph(&server)
            .await
            .hide_chat("19:a@thread.v2", "u-me", "t1", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unhide_posts_user_body() {
        let server = MockServer::start().await;
        expect_post(
            &server,
            "/chats/19:a@thread.v2/unhideForUser",
            json!({"user": {"id": "u-me", "tenantId": "t1"}}),
            204,
        )
        .await;
        graph(&server)
            .await
            .hide_chat("19:a@thread.v2", "u-me", "t1", false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn mark_unread_posts_user_body() {
        let server = MockServer::start().await;
        expect_post(
            &server,
            "/chats/19:a@thread.v2/markChatUnreadForUser",
            json!({"user": {"id": "u-me", "tenantId": "t1"}}),
            204,
        )
        .await;
        graph(&server)
            .await
            .mark_chat_unread("19:a@thread.v2", "u-me", "t1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn pin_binds_message_url() {
        let server = MockServer::start().await;
        expect_post(
            &server,
            "/chats/19:a@thread.v2/pinnedMessages",
            json!({"message@odata.bind": "https://graph.microsoft.com/v1.0/chats/19:a@thread.v2/messages/1700"}),
            201,
        )
        .await;
        graph(&server)
            .await
            .pin_message("19:a@thread.v2", "1700")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unpin_deletes_pin_path() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/chats/19:a@thread.v2/pinnedMessages/p1"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        graph(&server)
            .await
            .unpin_message("19:a@thread.v2", "p1")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn meeting_by_id_filters_and_returns_join_web_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/onlineMeetings"))
            .and(query_param(
                "$filter",
                "joinMeetingIdSettings/joinMeetingId eq '123456789012'",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [
                {"joinWebUrl": "https://teams.microsoft.com/l/meetup-join/x"},
                {"joinWebUrl": "https://teams.microsoft.com/l/meetup-join/y"}
            ]})))
            .expect(1)
            .mount(&server)
            .await;
        let found = graph(&server)
            .await
            .meeting_join_url_by_id("123456789012")
            .await
            .unwrap();
        assert_eq!(
            found.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/x")
        );
    }

    #[tokio::test]
    async fn meeting_by_id_none_and_apostrophe_doubled() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/onlineMeetings"))
            .and(query_param(
                "$filter",
                "joinMeetingIdSettings/joinMeetingId eq '1''2'",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            graph(&server)
                .await
                .meeting_join_url_by_id("1'2")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn pinned_messages_not_found_means_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/chats/19:a@thread.v2/pinnedMessages"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": {"code": "NotFound", "message": "Resource not found"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let pins = graph(&server)
            .await
            .pinned_messages("19:a@thread.v2", "me")
            .await
            .unwrap();
        assert!(pins.is_empty());
    }

    #[tokio::test]
    async fn pinned_preview_strips_html_and_caps_200_chars() {
        let server = MockServer::start().await;
        let long = "x".repeat(300);
        Mock::given(method("GET"))
            .and(path("/chats/19:a@thread.v2/pinnedMessages"))
            .and(query_param("$expand", "message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [
                {"id": "p1", "message": {"id": "m1", "createdDateTime": "2026-11-20T10:00:00Z",
                    "from": {"user": {"displayName": "Ada"}},
                    "body": {"contentType": "html", "content": "<p>Ship it</p>"}}},
                {"id": "p2", "message": {"id": "m2", "createdDateTime": "2026-11-20T10:00:00Z",
                    "from": {"user": {"displayName": "Bo"}},
                    "body": {"contentType": "html", "content": format!("<p>{long}</p>")}}},
                {"id": "p3"}
            ]})))
            .expect(1)
            .mount(&server)
            .await;
        let pins = graph(&server)
            .await
            .pinned_messages("19:a@thread.v2", "me")
            .await
            .unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(
            pins[0],
            PinnedMessage {
                id: "p1".into(),
                message_id: "m1".into(),
                author: "Ada".into(),
                preview: "Ship it".into(),
                at: crate::time::parse_iso("2026-11-20T10:00:00Z").unwrap(),
            }
        );
        assert_eq!(pins[1].preview.chars().count(), 200);
    }
}
