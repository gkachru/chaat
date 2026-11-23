use std::path::PathBuf;

use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use super::*;

const MESSAGES: &str = "messages";
use crate::app::PluginState;
use crate::backend::{Backend, Event, Merge, Waker};
use crate::model::{Delivery, Message};
use crate::paths::AppDirs;
use crate::plugin::registry;
use crate::settings::{PluginEntry, Settings};

const ACCOUNT: &str = "u.t";
const CHAT: &str = "19:a@thread.v2";

struct Rig {
    app: App,
    service: UnboundedReceiver<Command>,
    account: UnboundedReceiver<Command>,
    ctx: egui::Context,
}

fn plugin(app: &mut App, id: &str, approved: bool) {
    app.settings.plugins.push(PluginEntry {
        id: id.into(),
        path: PathBuf::from(format!("{id}.exe")),
        enabled: true,
        needs: vec![MESSAGES.into()],
        approved_needs: if approved {
            vec![MESSAGES.into()]
        } else {
            vec![]
        },
        ..PluginEntry::default()
    });
    app.plugins.insert(
        id.into(),
        PluginState {
            running: true,
            offers: vec![registry::AGENTS],
            ready_seen: true,
            ..PluginState::default()
        },
    );
}

fn rig(approved: bool) -> Rig {
    let (backend, account) = Backend::recording();
    let dir = std::env::temp_dir().join("chaat-test-unused");
    let mut app = App::with_backend(
        AppDirs::under(&dir),
        Settings::default(),
        Waker::default(),
        backend,
        false,
    );
    // Incoming messages would otherwise show real desktop toasts.
    app.settings.notifications = false;
    let (service, service_rx) = Backend::recording();
    app.service = service;
    app.id = Some(ACCOUNT.into());
    plugin(&mut app, "a", approved);
    Rig {
        app,
        service: service_rx,
        account,
        ctx: egui::Context::default(),
    }
}

fn open_chat(app: &mut App, propose: bool) {
    app.settings.agent_access.insert(
        ACCOUNT.into(),
        AgentAccess {
            chats: Access::Selected([format!("chat:{CHAT}")].into()),
            propose,
            ..AgentAccess::default()
        },
    );
}

fn conversation() -> ConversationId {
    ConversationId::Chat(CHAT.into())
}

fn lines(rx: &mut UnboundedReceiver<Command>) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    while let Ok(command) = rx.try_recv() {
        if let Command::PluginSend { id, message } = command {
            let line = message.to_line();
            out.push((id, serde_json::from_str(&line).unwrap()));
        }
    }
    out
}

fn query(app: &mut App, account: &str) {
    app.on_agent_query("a", 7, account.into(), AgentQuery::ListChannels);
}

fn reply(text: &str) -> AgentProposal {
    AgentProposal::Reply {
        conversation: format!("chat:{CHAT}"),
        text: text.into(),
        reply_to: None,
        agent: "Helper".into(),
    }
}

fn propose(app: &mut App, request: u64, proposal: AgentProposal) {
    app.on_agent_propose("a", request, ACCOUNT.into(), proposal);
}

fn accept(id: &str) -> Action {
    Action::Agent(AgentAction::Send {
        proposal: id.into(),
    })
}

#[test]
fn agent_events_need_messages() {
    let event = || HostMessage::AgentEvent {
        account: ACCOUNT.into(),
        event: json!({"type": "new_message"}),
    };
    let mut rig = rig(false);
    rig.app.agent_send("a", event());
    assert!(lines(&mut rig.service).is_empty());
    // An answer to the plugin's own request is never held back.
    rig.app.agent_answer("a", 1, Err("not_allowed"));
    assert_eq!(lines(&mut rig.service).len(), 1);
    let mut rig = self::rig(true);
    rig.app.agent_send("a", event());
    assert_eq!(lines(&mut rig.service).len(), 1);
    rig.app.plugins.get_mut("a").unwrap().running = false;
    rig.app.agent_send("a", event());
    assert!(lines(&mut rig.service).is_empty());
}

#[test]
fn query_needs_messages_need() {
    let mut rig = rig(false);
    query(&mut rig.app, ACCOUNT);
    let sent = lines(&mut rig.service);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1["type"], "agent_result");
    assert_eq!(sent[0].1["ok"], false);
    assert_eq!(sent[0].1["data"]["error"], "not_allowed");
    assert!(rig.account.try_recv().is_err());

    rig.app.settings.plugins[0].approved_needs = vec![MESSAGES.into()];
    query(&mut rig.app, ACCOUNT);
    assert!(matches!(
        rig.account.try_recv(),
        Ok(Command::AgentRead { request: 7, .. })
    ));
    assert!(lines(&mut rig.service).is_empty());
}

#[test]
fn unknown_account() {
    let mut rig = rig(true);
    query(&mut rig.app, "other.t");
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].1["data"]["error"], "no_account");
    assert!(rig.account.try_recv().is_err());
}

#[test]
fn only_first_agents_plugin() {
    let mut rig = rig(true);
    plugin(&mut rig.app, "b", true);
    rig.app
        .on_agent_query("b", 3, ACCOUNT.into(), AgentQuery::ListChannels);
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].0, "b");
    assert_eq!(sent[0].1["data"]["error"], "not_allowed");
    assert!(rig.account.try_recv().is_err());
    open_chat(&mut rig.app, true);
    rig.app.on_agent_propose("b", 4, ACCOUNT.into(), reply("x"));
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].1["data"]["error"], "not_allowed");
    assert!(
        !rig.app
            .agent_proposals
            .has_pending(ACCOUNT, &conversation())
    );
}

#[test]
fn answer_is_routed_back_as_agent_result() {
    let mut rig = rig(true);
    rig.app
        .on_agent_answer("a".into(), 9, Ok(json!({"chats": []})));
    rig.app.on_agent_answer("a".into(), 10, Err("not_found"));
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].1["id"], 9);
    assert_eq!(sent[0].1["ok"], true);
    assert_eq!(sent[1].1["data"]["error"], "not_found");
}

#[test]
fn proposals_need_propose_access() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, false);
    propose(&mut rig.app, 1, reply("x"));
    assert_eq!(lines(&mut rig.service)[0].1["data"]["error"], "not_allowed");
    assert!(
        !rig.app
            .agent_proposals
            .has_pending(ACCOUNT, &conversation())
    );
}

#[test]
fn notice_no_raise() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    rig.app.wants_show = false;
    propose(&mut rig.app, 1, reply("secret text"));
    assert!(!rig.app.wants_show);
    assert_eq!(rig.app.toasts.len(), 1);
    assert!(rig.app.toasts[0].text.contains("proposes a reply in"));
    assert!(!rig.app.toasts[0].text.contains("secret"));
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].1["ok"], true);
    assert_eq!(sent[0].1["data"]["state"], "pending");
    assert_eq!(sent[0].1["data"]["proposal_id"], "p1");
}

#[test]
fn superseded_answered() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("one"));
    propose(&mut rig.app, 2, reply("two"));
    let sent = lines(&mut rig.service);
    let replaced = sent
        .iter()
        .find(|(_, v)| v["type"] == "agent_outcome")
        .unwrap();
    assert_eq!(replaced.1["proposal"], "p1");
    assert_eq!(replaced.1["outcome"], "replaced");
}

#[test]
fn send_goes_optimistic() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("hello"));
    lines(&mut rig.service);
    rig.app.apply(accept("p1"), &rig.ctx);
    let messages = &rig.app.conversations[&conversation()].messages;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].delivery, Delivery::Sending);
    // The send is saved to the outbox first, then sent.
    assert!(matches!(rig.account.try_recv(), Ok(Command::SaveOutbox(_))));
    assert!(matches!(rig.account.try_recv(), Ok(Command::Send { .. })));
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].1["outcome"], "sent");
    assert!(
        !rig.app
            .agent_proposals
            .has_pending(ACCOUNT, &conversation())
    );
}

#[test]
fn own_send_outdates() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("hello"));
    lines(&mut rig.service);
    rig.app.apply(
        Action::Send {
            conversation: conversation(),
            text: "mine".into(),
            reply_to: None,
            mentions: Vec::new(),
            files: Vec::new(),
            images: Vec::new(),
        },
        &rig.ctx,
    );
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].1["proposal"], "p1");
    assert_eq!(sent[0].1["outcome"], "outdated");
    assert!(rig.app.agent_proposals.get("p1").unwrap().outdated);
    // An outdated reply cannot be sent as is.
    rig.app.apply(accept("p1"), &rig.ctx);
    assert_eq!(rig.app.conversations[&conversation()].messages.len(), 1);
}

fn incoming(id: &str, at: i64) -> Message {
    Message {
        id: id.into(),
        author_id: Some("someone".into()),
        author: "Sam".into(),
        created: at,
        ..Default::default()
    }
}

fn land(
    app: &mut App,
    ctx: &egui::Context,
    chat: ConversationId,
    at: i64,
    merge: Merge,
    cached: bool,
) {
    app.handle(
        Event::Messages {
            conversation: chat,
            messages: vec![incoming("2", at)],
            merge,
            more: None,
            cached,
        },
        ctx,
    );
}

#[test]
fn events_only_allowed() {
    let mut rig = rig(true);
    let ctx = rig.ctx.clone();
    let other = ConversationId::Chat("19:b@thread.v2".into());
    for chat in [conversation(), other.clone()] {
        rig.app.handle(
            Event::Messages {
                conversation: chat,
                messages: vec![incoming("1", 1000)],
                merge: Merge::Latest,
                more: None,
                cached: false,
            },
            &ctx,
        );
    }
    open_chat(&mut rig.app, false);
    lines(&mut rig.service);
    land(&mut rig.app, &ctx, other, 2000, Merge::Latest, false);
    land(
        &mut rig.app,
        &ctx,
        conversation(),
        2000,
        Merge::Latest,
        true,
    );
    land(&mut rig.app, &ctx, conversation(), 1, Merge::Older, false);
    assert!(lines(&mut rig.service).is_empty());
    land(
        &mut rig.app,
        &ctx,
        conversation(),
        2000,
        Merge::Latest,
        false,
    );
    let sent = lines(&mut rig.service);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1["type"], "agent_event");
    assert_eq!(sent[0].1["account"], ACCOUNT);
    assert_eq!(sent[0].1["event"]["type"], "new_message");
    assert_eq!(
        sent[0].1["event"]["conversation_id"],
        format!("chat:{CHAT}")
    );
    assert_eq!(sent[0].1["event"]["message"]["message_id"], "2");
}

#[test]
fn user_sent_event_after_the_send_lands() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, false);
    let message = Message {
        id: "real".into(),
        author_id: Some("me".into()),
        ..Default::default()
    };
    rig.app
        .on_sent(conversation(), "local-1".into(), message.clone());
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].1["event"]["type"], "user_sent");
    assert_eq!(sent[0].1["event"]["message_id"], "real");
    rig.app.on_sent(
        ConversationId::Chat("19:other".into()),
        "local-2".into(),
        message,
    );
    assert!(lines(&mut rig.service).is_empty());
}

#[test]
fn edit_fills_draft() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("draft me"));
    lines(&mut rig.service);
    rig.app.apply(
        Action::Agent(AgentAction::Edit {
            proposal: "p1".into(),
        }),
        &rig.ctx,
    );
    assert_eq!(rig.app.drafts[&conversation()].text, "draft me");
    assert_eq!(lines(&mut rig.service)[0].1["outcome"], "edited");
    assert!(rig.account.try_recv().is_err());
}

fn load(app: &mut App, ids: &[&str]) {
    app.conversations
        .entry(conversation())
        .or_default()
        .messages = ids
        .iter()
        .map(|id| Message {
            id: (*id).into(),
            ..Default::default()
        })
        .collect();
}

fn reaction_to(message: &str) -> AgentProposal {
    AgentProposal::Reaction {
        conversation: format!("chat:{CHAT}"),
        message: message.into(),
        emoji: "like".into(),
        agent: "Helper".into(),
    }
}

#[test]
fn reaction_to_unknown_message_not_found() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    // Conversation not loaded.
    propose(&mut rig.app, 1, reaction_to("m1"));
    // Loaded, message absent.
    load(&mut rig.app, &["m2"]);
    propose(&mut rig.app, 2, reaction_to("m1"));
    let sent = lines(&mut rig.service);
    assert_eq!(sent.len(), 2);
    for (_, line) in &sent {
        assert_eq!(line["ok"], false);
        assert_eq!(line["data"]["error"], "not_found");
    }
    assert!(
        !rig.app
            .agent_proposals
            .has_pending(ACCOUNT, &conversation())
    );
}

#[test]
fn accept_after_delete_outdated() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    load(&mut rig.app, &["m1"]);
    propose(&mut rig.app, 1, reaction_to("m1"));
    lines(&mut rig.service);
    load(&mut rig.app, &[]);
    rig.app.apply(
        Action::Agent(AgentAction::AcceptReaction {
            proposal: "p1".into(),
        }),
        &rig.ctx,
    );
    let sent = lines(&mut rig.service);
    assert_eq!(sent[0].1["outcome"], "outdated");
    assert!(rig.account.try_recv().is_err());
    assert!(rig.app.agent_proposals.get("p1").is_none());
}

#[test]
fn discard_and_accept_reaction() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    load(&mut rig.app, &["m1", "m2"]);
    let reaction = |message: &str| AgentProposal::Reaction {
        conversation: format!("chat:{CHAT}"),
        message: message.into(),
        emoji: "like".into(),
        agent: "Helper".into(),
    };
    propose(&mut rig.app, 1, reaction("m1"));
    propose(&mut rig.app, 2, reaction("m2"));
    lines(&mut rig.service);
    rig.app.apply(
        Action::Agent(AgentAction::Discard {
            proposal: "p1".into(),
        }),
        &rig.ctx,
    );
    rig.app.apply(
        Action::Agent(AgentAction::AcceptReaction {
            proposal: "p2".into(),
        }),
        &rig.ctx,
    );
    let outcomes: Vec<String> = lines(&mut rig.service)
        .iter()
        .map(|(_, v)| v["outcome"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(outcomes, ["discarded", "accepted"]);
    assert!(matches!(
        rig.account.try_recv(),
        Ok(Command::React { on: true, .. })
    ));
}

#[test]
fn access_actions_edit_settings() {
    let mut rig = rig(true);
    let on = |on| {
        Action::Agent(AgentAction::LetRead {
            conversation: conversation(),
            on,
        })
    };
    rig.app.apply(on(true), &rig.ctx);
    assert!(rig.app.agent_access_of(ACCOUNT).can_read(&conversation()));
    rig.app.apply(on(false), &rig.ctx);
    assert!(!rig.app.agent_access_of(ACCOUNT).can_read(&conversation()));
    let all = AgentAccess {
        chats: Access::All,
        ..AgentAccess::default()
    };
    rig.app.apply(
        Action::Agent(AgentAction::SetAccess {
            account: ACCOUNT.into(),
            access: all.clone(),
        }),
        &rig.ctx,
    );
    assert_eq!(rig.app.agent_access_of(ACCOUNT), all);
    rig.app.apply(on(true), &rig.ctx);
    assert_eq!(rig.app.agent_access_of(ACCOUNT), all);
}

#[test]
fn plugin_receives_lists_messages() {
    let mut rig = rig(false);
    assert!(!rig.app.plugin_receives("a").contains(&"messages"));
    rig.app.settings.plugins[0].approved_needs = vec![MESSAGES.into()];
    assert_eq!(rig.app.plugin_receives("a"), ["messages"]);
}

#[test]
fn proposals_do_not_outlive_their_plugin_or_access() {
    // Plugin stopped: the card goes and a late Send does nothing.
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("hello"));
    rig.app.plugins.get_mut("a").unwrap().running = false;
    rig.app.apply(accept("p1"), &rig.ctx);
    assert!(!rig.app.conversations.contains_key(&conversation()));
    assert!(rig.app.agent_proposals.get("p1").is_none());
    assert!(rig.account.try_recv().is_err());

    // Plugin removed.
    let mut rig = self::rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("hello"));
    rig.app.remove_plugin("a");
    rig.app.prune_agent_proposals();
    assert!(rig.app.agent_proposals.get("p1").is_none());

    // The chat is taken off the list: dropped, and the owner is told.
    let mut rig = self::rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("hello"));
    lines(&mut rig.service);
    rig.app.apply(
        Action::Agent(AgentAction::LetRead {
            conversation: conversation(),
            on: false,
        }),
        &rig.ctx,
    );
    assert!(rig.app.agent_proposals.get("p1").is_none());
    let told = lines(&mut rig.service);
    assert_eq!(told[0].1["outcome"], "discarded");

    // The person revokes the messages need.
    let mut rig = self::rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("hello"));
    rig.app.settings.plugins[0].approved_needs.clear();
    rig.app.apply(accept("p1"), &rig.ctx);
    assert!(rig.app.agent_proposals.get("p1").is_none());
    assert!(!rig.app.conversations.contains_key(&conversation()));
}

#[test]
fn proposal_of_a_gone_account_is_dropped() {
    let mut rig = rig(true);
    open_chat(&mut rig.app, true);
    propose(&mut rig.app, 1, reply("hello"));
    rig.app.accounts[0].id = Some("other.t".into());
    lines(&mut rig.service);
    rig.app.apply(accept("p1"), &rig.ctx);
    assert!(rig.app.agent_proposals.get("p1").is_none());
    assert!(
        lines(&mut rig.service)
            .iter()
            .all(|(_, line)| line["outcome"] != "sent")
    );
}

#[test]
fn oversized_new_message_event_stays_under_the_line_limit() {
    let mut rig = rig(true);
    let ctx = rig.ctx.clone();
    rig.app.handle(
        Event::Messages {
            conversation: conversation(),
            messages: vec![incoming("1", 1000)],
            merge: Merge::Latest,
            more: None,
            cached: false,
        },
        &ctx,
    );
    open_chat(&mut rig.app, false);
    lines(&mut rig.service);
    let mut big = incoming("2", 2000);
    big.body = crate::html::RichText::plain(&"x".repeat(200_000));
    rig.app.handle(
        Event::Messages {
            conversation: conversation(),
            messages: vec![big],
            merge: Merge::Latest,
            more: None,
            cached: false,
        },
        &ctx,
    );
    let mut sent = Vec::new();
    while let Ok(Command::PluginSend { message, .. }) = rig.service.try_recv() {
        sent.push(message.to_line());
    }
    let event = sent.iter().find(|l| l.contains("new_message")).unwrap();
    assert!(
        event.len() < crate::plugin::protocol::MAX_LINE,
        "{}",
        event.len()
    );
}
