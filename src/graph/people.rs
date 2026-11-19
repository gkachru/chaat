//! People and the signed-in person's presence: directory search, new
//! chats, availability and status message.

use serde_json::json;

use super::types as g;
use super::{Graph, GraphError, convert, segment};
use crate::model::{Availability, Chat, Member, Presence};

impl Graph {
    /// Directory search by name or email (User.ReadBasic.All). `$search`
    /// on users needs the `ConsistencyLevel: eventual` header.
    pub async fn search_people(&self, query: &str) -> Result<Vec<Member>, GraphError> {
        // Quotes and backslashes would end the search clause early.
        let cleaned: String = query
            .chars()
            .filter(|c| !matches!(c, '"' | '\\'))
            .collect::<String>()
            .trim()
            .to_owned();
        if cleaned.chars().count() < 2 {
            return Ok(Vec::new());
        }
        let search = format!("\"displayName:{cleaned}\" OR \"mail:{cleaned}\"");
        let url = format!(
            "users?$search={}&$select=id,displayName,mail,userPrincipalName&$top=15&$count=true",
            urlencoding::encode(&search)
        );
        let page: g::Page<g::User> = self
            .get_with(&url, &[("ConsistencyLevel", "eventual")])
            .await?;
        Ok(page
            .value
            .into_iter()
            .map(|user| Member {
                tenant_id: None,
                email: user.mail.or(user.user_principal_name),
                display_name: user.display_name.unwrap_or_default(),
                user_id: user.id,
            })
            .filter(|member| !member.display_name.is_empty())
            .collect())
    }

    /// `POST /chats`: a one-on-one chat with one other person, or a group
    /// chat with several (Chat.Create). Graph returns the existing chat
    /// when a one-on-one chat with that person already exists.
    pub async fn create_chat(
        &self,
        me: &str,
        others: &[String],
        topic: Option<&str>,
    ) -> Result<Chat, GraphError> {
        let member = |id: &str| {
            json!({
                "@odata.type": "#microsoft.graph.aadUserConversationMember",
                "roles": ["owner"],
                "user@odata.bind": format!("https://graph.microsoft.com/v1.0/users('{id}')"),
            })
        };
        let mut members = vec![member(me)];
        members.extend(others.iter().map(|id| member(id)));
        let mut body = json!({
            "chatType": if others.len() == 1 { "oneOnOne" } else { "group" },
            "members": members,
        });
        if others.len() > 1
            && let Some(topic) = topic.map(str::trim).filter(|t| !t.is_empty())
        {
            body["topic"] = json!(topic);
        }
        let created: g::Chat = self.post("chats", &body).await?;
        // The answer has no members; read the chat back with them.
        match self
            .get::<g::Chat>(&format!("chats/{}?$expand=members", segment(&created.id)))
            .await
        {
            Ok(full) => Ok(convert::chat(full, self.my_id())),
            Err(_) => Ok(convert::chat(created, self.my_id())),
        }
    }

    /// `GET /chats/{id}/members`: every member with the membership id
    /// (Chat.ReadWrite). The list has no paging, unlike `$expand=members`,
    /// which stops at 25. Members without a user id (anonymous, guests of
    /// other kinds) are dropped, as in `convert::chat`.
    pub async fn chat_members(&self, chat: &str) -> Result<Vec<(String, Member)>, GraphError> {
        let page: g::Page<g::ConversationMember> = self
            .get(&format!("chats/{}/members", segment(chat)))
            .await?;
        Ok(page
            .value
            .into_iter()
            .filter_map(|member| {
                Some((
                    member.id?,
                    Member {
                        user_id: member.user_id?,
                        display_name: member.display_name.unwrap_or_default(),
                        email: member.email,
                        tenant_id: member.tenant_id,
                    },
                ))
            })
            .collect())
    }

    /// `POST /chats/{id}/members`: one person per request. With `history`
    /// they see all earlier messages; without it, only new ones.
    pub async fn add_chat_member(
        &self,
        chat: &str,
        user: &str,
        history: bool,
    ) -> Result<(), GraphError> {
        let mut body = json!({
            "@odata.type": "#microsoft.graph.aadUserConversationMember",
            "roles": ["owner"],
            "user@odata.bind": format!("https://graph.microsoft.com/v1.0/users('{user}')"),
        });
        if history {
            body["visibleHistoryStartDateTime"] = json!("0001-01-01T00:00:00Z");
        }
        self.post_no_content(&format!("chats/{}/members", segment(chat)), &body)
            .await
    }

    /// `DELETE /chats/{id}/members/{membership-id}` (ChatMember.ReadWrite,
    /// admin consent). The membership id is opaque.
    pub async fn remove_chat_member(&self, chat: &str, membership: &str) -> Result<(), GraphError> {
        self.delete(&format!(
            "chats/{}/members/{}",
            segment(chat),
            segment(membership)
        ))
        .await
    }

    /// `GET /me/presence` (Presence.Read, part of Presence.Read.All).
    pub async fn my_presence(&self) -> Result<Presence, GraphError> {
        let raw: g::Presence = self.get("me/presence").await?;
        Ok(convert::presence(raw).1)
    }

    /// `setUserPreferredPresence`, or `clearUserPreferredPresence` for
    /// `None` (Presence.ReadWrite). It shows while the person has any
    /// presence session: a Teams client, or Chaat's own.
    pub async fn set_preferred_presence(
        &self,
        me: &str,
        availability: Option<Availability>,
    ) -> Result<(), GraphError> {
        match availability.and_then(Availability::graph_preferred) {
            Some((availability, activity)) => {
                self.post_no_content(
                    &format!("users/{}/presence/setUserPreferredPresence", segment(me)),
                    &json!({ "availability": availability, "activity": activity }),
                )
                .await
            }
            None => {
                self.post_no_content(
                    &format!("users/{}/presence/clearUserPreferredPresence", segment(me)),
                    &json!({}),
                )
                .await
            }
        }
    }

    /// `setStatusMessage`; an empty text clears it (Presence.ReadWrite).
    pub async fn set_status_message(&self, me: &str, text: &str) -> Result<(), GraphError> {
        self.post_no_content(
            &format!("users/{}/presence/setStatusMessage", segment(me)),
            &json!({ "statusMessage": { "message": { "content": text.trim(), "contentType": "text" } } }),
        )
        .await
    }

    /// `setPresence`: Chaat's own presence session, as a Teams client keeps
    /// one while it runs. `sessionId` is the app's client ID.
    pub async fn set_presence_session(
        &self,
        me: &str,
        client_id: &str,
        away: bool,
        minutes: u32,
    ) -> Result<(), GraphError> {
        let (availability, activity) = if away {
            ("Away", "Away")
        } else {
            ("Available", "Available")
        };
        self.post_no_content(
            &format!("users/{}/presence/setPresence", segment(me)),
            &json!({
                "sessionId": client_id,
                "availability": availability,
                "activity": activity,
                "expirationDuration": format!("PT{}M", minutes.clamp(5, 240)),
            }),
        )
        .await
    }

    /// `clearPresence` for Chaat's session.
    pub async fn clear_presence_session(
        &self,
        me: &str,
        client_id: &str,
    ) -> Result<(), GraphError> {
        self.post_no_content(
            &format!("users/{}/presence/clearPresence", segment(me)),
            &json!({ "sessionId": client_id }),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn graph(server: &MockServer) -> Graph {
        Graph::with_base(
            reqwest::Client::new(),
            testing::session("at", &server.uri()).await,
            &server.uri(),
        )
    }

    #[tokio::test]
    async fn people_search_is_eventual_and_escaped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users"))
            .and(header("consistencylevel", "eventual"))
            .and(query_param("$search", "\"displayName:ada\" OR \"mail:ada\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": [
                {"id": "u-ada", "displayName": "Ada Lovelace", "mail": "ada@contoso.com"},
                {"id": "u-x", "displayName": "Ada Room", "userPrincipalName": "room@contoso.com"}
            ]})))
            .expect(1)
            .mount(&server)
            .await;
        let graph = graph(&server).await;
        let people = graph.search_people("a\"da").await.unwrap();
        assert_eq!(people.len(), 2);
        assert_eq!(people[1].email.as_deref(), Some("room@contoso.com"));
        assert!(graph.search_people("a").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn new_chats_include_me_and_bind_users() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chats"))
            .and(body_json(serde_json::json!({
                "chatType": "group",
                "topic": "Launch",
                "members": [
                    {"@odata.type": "#microsoft.graph.aadUserConversationMember", "roles": ["owner"], "user@odata.bind": "https://graph.microsoft.com/v1.0/users('u-me')"},
                    {"@odata.type": "#microsoft.graph.aadUserConversationMember", "roles": ["owner"], "user@odata.bind": "https://graph.microsoft.com/v1.0/users('u-ada')"},
                    {"@odata.type": "#microsoft.graph.aadUserConversationMember", "roles": ["owner"], "user@odata.bind": "https://graph.microsoft.com/v1.0/users('u-grace')"}
                ]
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": "19:new@thread.v2", "chatType": "group", "topic": "Launch"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/chats/19:new@thread.v2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "19:new@thread.v2", "chatType": "group", "topic": "Launch",
                "members": [{"userId": "u-me", "displayName": "Me"}, {"userId": "u-ada", "displayName": "Ada"}, {"userId": "u-grace", "displayName": "Grace"}]
            })))
            .mount(&server)
            .await;
        let chat = graph(&server)
            .await
            .create_chat("u-me", &["u-ada".into(), "u-grace".into()], Some("Launch"))
            .await
            .unwrap();
        assert_eq!(chat.members.len(), 3);
        assert_eq!(chat.title("u-me"), "Launch");
    }

    #[tokio::test]
    async fn presence_writes_hit_their_endpoints() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/users/u-me/presence/setUserPreferredPresence"))
            .and(body_json(
                serde_json::json!({"availability": "DoNotDisturb", "activity": "DoNotDisturb"}),
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/users/u-me/presence/clearUserPreferredPresence"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/users/u-me/presence/setStatusMessage"))
            .and(body_json(serde_json::json!({"statusMessage": {"message": {"content": "Back at 3", "contentType": "text"}}})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/users/u-me/presence/setPresence"))
            .and(body_json(serde_json::json!({"sessionId": "client", "availability": "Available", "activity": "Available", "expirationDuration": "PT30M"})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let graph = graph(&server).await;
        graph
            .set_preferred_presence("u-me", Some(Availability::DoNotDisturb))
            .await
            .unwrap();
        graph.set_preferred_presence("u-me", None).await.unwrap();
        graph
            .set_status_message("u-me", " Back at 3 ")
            .await
            .unwrap();
        graph
            .set_presence_session("u-me", "client", false, 30)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn members_list_maps_30_members() {
        let server = MockServer::start().await;
        let value: Vec<_> = (0..30)
            .map(|i| serde_json::json!({"id": format!("m{i}"), "userId": format!("u{i}"), "displayName": format!("P{i}")}))
            .chain([serde_json::json!({"id": "mx", "displayName": "No user"})])
            .collect();
        Mock::given(method("GET"))
            .and(path("/chats/19:a@thread.v2/members"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "value": value })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let members = graph(&server)
            .await
            .chat_members("19:a@thread.v2")
            .await
            .unwrap();
        assert_eq!(members.len(), 30);
        assert_eq!(members[0].0, "m0");
        assert_eq!(members[29].1.user_id, "u29");
        assert_eq!(members[29].1.display_name, "P29");
    }

    #[tokio::test]
    async fn add_member_without_history_omits_field() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chats/19:a@thread.v2/members"))
            .and(body_json(serde_json::json!({
                "@odata.type": "#microsoft.graph.aadUserConversationMember",
                "roles": ["owner"],
                "user@odata.bind": "https://graph.microsoft.com/v1.0/users('u2')"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": "m2"})))
            .expect(1)
            .mount(&server)
            .await;
        graph(&server)
            .await
            .add_chat_member("19:a@thread.v2", "u2", false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn add_member_with_all_history() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chats/19:a@thread.v2/members"))
            .and(body_json(serde_json::json!({
                "@odata.type": "#microsoft.graph.aadUserConversationMember",
                "roles": ["owner"],
                "user@odata.bind": "https://graph.microsoft.com/v1.0/users('u2')",
                "visibleHistoryStartDateTime": "0001-01-01T00:00:00Z"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": "m2"})))
            .expect(1)
            .mount(&server)
            .await;
        graph(&server)
            .await
            .add_chat_member("19:a@thread.v2", "u2", true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn remove_member_deletes_by_membership_id() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/chats/19:a@thread.v2/members/m%231%3D%2F"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        graph(&server)
            .await
            .remove_chat_member("19:a@thread.v2", "m#1=/")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn remove_member_403_is_forbidden() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(403).set_body_json(
                serde_json::json!({"error": {"code": "Forbidden", "message": "secret"}}),
            ))
            .mount(&server)
            .await;
        let error = graph(&server)
            .await
            .remove_chat_member("19:a@thread.v2", "m1")
            .await
            .unwrap_err();
        assert!(matches!(error, GraphError::Forbidden { .. }), "{error:?}");
    }
}
