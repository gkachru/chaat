//! Agent reads through the real backend: answered from the cache, and never
//! a request to Graph.

use std::sync::Arc;
use std::time::Duration;

use chaat::agents::access::{Access, AgentAccess};
use chaat::backend::{AuthStatus, Backend, BackendOptions, Command, Event, Waker};
use chaat::model::ConversationId;
use chaat::paths::AppDirs;
use chaat::plugin::agent::AgentQuery;
use chaat::secrets::MemoryStore;
use chaat::sync::{Resource, Trigger};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::{id_token, query_value, wait_for};

const CLIENT: &str = "6731de76-14a6-49ae-97bc-6eba6914391e";
const CHAT: &str = "19:ada-me@unq.gbl.spaces";

async fn mock_graph() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "id_token": id_token(),
            "expires_in": 3600,
            "scope": "User.Read Chat.ReadWrite ChatMessage.Send"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1.0/me/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{
            "id": CHAT, "chatType": "oneOnOne", "lastUpdatedDateTime": "2026-10-11T10:00:00Z",
            "members": [
                {"userId": "u-ada", "displayName": "Ada Lovelace", "email": "ada@contoso.com"},
                {"userId": "u-me", "displayName": "Alex Rivera", "email": "alex@contoso.com"}
            ],
            "lastMessagePreview": {"id": "1700", "createdDateTime": "2026-10-11T10:00:00Z", "messageType": "message",
                "body": {"contentType": "html", "content": "<p>Ship it?</p>"},
                "from": {"user": {"id": "u-ada", "displayName": "Ada Lovelace"}}},
            "viewpoint": {"lastMessageReadDateTime": "2026-10-11T09:00:00Z"}
        }]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v1.0/chats/{CHAT}/messages")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [
            {"id": "1700", "messageType": "message", "createdDateTime": "2026-10-11T10:00:00Z",
             "lastModifiedDateTime": "2026-10-11T10:00:00Z",
             "from": {"user": {"id": "u-ada", "displayName": "Ada Lovelace"}},
             "body": {"contentType": "html", "content": "<p>Ship it?</p>"}},
            {"id": "1690", "messageType": "message", "createdDateTime": "2026-10-11T09:50:00Z",
             "lastModifiedDateTime": "2026-10-11T09:50:00Z",
             "from": {"user": {"id": "u-me", "displayName": "Alex Rivera"}},
             "body": {"contentType": "html", "content": "<p>Build is green</p>"}}
        ]})))
        .mount(&server)
        .await;
    server
}

fn ask(backend: &Backend, request: u64, query: AgentQuery, access: &AgentAccess) -> Value {
    backend.send(Command::AgentRead {
        plugin: "agents".into(),
        request,
        query,
        access: access.clone(),
    });
    let data = wait_for(backend, 5, |event| match event {
        Event::AgentAnswer {
            plugin,
            request: id,
            data,
        } if id == request => {
            assert_eq!(plugin, "agents");
            Some(data)
        }
        _ => None,
    });
    match data {
        Ok(value) => value,
        Err(error) => json!({"error": error}),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_fetch_on_agent_read() {
    let server = mock_graph().await;
    let dir = tempfile::tempdir().unwrap();
    let options = BackendOptions {
        authority: server.uri(),
        graph_base: format!("{}/v1.0", server.uri()),
        secrets: Arc::new(MemoryStore::default()),
        open_browser: false,
        push_renew_every: chaat::realtime::RENEW_EVERY,
        relay_connect_timeout: Duration::from_secs(20),
    };
    let backend = Backend::spawn_with(AppDirs::under(dir.path()), Waker::default(), options);
    backend.send(Command::Start {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        account: None,
    });
    wait_for(&backend, 5, |e| {
        matches!(e, Event::Auth(AuthStatus::SignedOut)).then_some(())
    });
    backend.send(Command::SignIn {
        extra_scopes: Vec::new(),
        device_code: false,
    });
    let url = wait_for(&backend, 5, |e| match e {
        Event::Auth(AuthStatus::WaitingForBrowser { url }) => Some(url),
        _ => None,
    });
    let redirect = query_value(&url, "redirect_uri");
    let state = query_value(&url, "state");
    let port = redirect.trim_start_matches("http://localhost:");
    reqwest::get(format!(
        "http://127.0.0.1:{port}/?code=the-code&state={state}"
    ))
    .await
    .unwrap();
    wait_for(&backend, 10, |e| {
        matches!(e, Event::Auth(AuthStatus::SignedIn { .. })).then_some(())
    });
    // Seed the cache the ordinary way: the person opens the chat.
    backend.send(Command::Fetch {
        resource: Resource::Chats,
        trigger: Trigger::Start,
    });
    wait_for(&backend, 10, |e| {
        matches!(e, Event::Chats { cached: false, .. }).then_some(())
    });
    let conversation = ConversationId::Chat(CHAT.into());
    backend.send(Command::Fetch {
        resource: Resource::Messages(conversation.clone()),
        trigger: Trigger::Opened,
    });
    wait_for(&backend, 10, |e| {
        matches!(e, Event::Messages { cached: false, .. }).then_some(())
    });
    // Let the start-up work settle before counting.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = server.received_requests().await.unwrap().len();

    let access = AgentAccess {
        chats: Access::All,
        channels: Access::All,
        propose: false,
    };
    let key = conversation.key();
    let list = ask(&backend, 1, AgentQuery::ListChats { limit: 50 }, &access);
    assert_eq!(list["chats"][0]["title"], "Ada Lovelace");
    assert_eq!(list["chats"][0]["id"], key);
    let channels = ask(&backend, 2, AgentQuery::ListChannels, &access);
    assert!(channels["channels"].as_array().unwrap().is_empty());
    let read = ask(
        &backend,
        3,
        AgentQuery::ReadMessages {
            conversation: key.clone(),
            before: None,
            limit: 10,
        },
        &access,
    );
    assert_eq!(read["messages"].as_array().unwrap().len(), 2);
    assert_eq!(read["messages"][0]["text"], "Build is green");
    let recent = ask(
        &backend,
        4,
        AgentQuery::Recent {
            since: 0,
            limit: 10,
            unread_only: true,
        },
        &access,
    );
    assert_eq!(recent["messages"][0]["conversation"], "Ada Lovelace");
    let hits = ask(
        &backend,
        5,
        AgentQuery::Search {
            query: "green".into(),
            limit: 10,
        },
        &access,
    );
    assert_eq!(hits["hits"][0]["message_id"], "1690");
    // Closed access answers the same questions with nothing or an error.
    let closed = AgentAccess::default();
    let denied = ask(
        &backend,
        6,
        AgentQuery::ReadMessages {
            conversation: key,
            before: None,
            limit: 10,
        },
        &closed,
    );
    assert_eq!(denied["error"], "not_allowed");
    let unknown = ask(
        &backend,
        7,
        AgentQuery::ReadMessages {
            conversation: "chat:19:nobody".into(),
            before: None,
            limit: 10,
        },
        &access,
    );
    assert_eq!(unknown["error"], "not_found");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = server.received_requests().await.unwrap().len();
    assert_eq!(before, after, "an agent read reached Graph");
}
