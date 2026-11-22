//! Pinned messages and links to messages.

use super::App;
use crate::backend::Command;
use crate::model::{ConversationId, Message, NOTES_CHAT, PinnedMessage, ToastKind};
use crate::sync::{Resource, Trigger};

impl App {
    pub(super) fn pin_message(&mut self, chat: String, message: String) {
        self.backend.send(Command::PinMessage { chat, message });
    }

    pub(super) fn unpin_message(&mut self, chat: String, pin: String) {
        if let Some(pins) = self.pins.get_mut(&chat) {
            pins.retain(|p| p.id != pin);
        }
        self.backend.send(Command::UnpinMessage { chat, pin });
    }

    pub(super) fn copy_message_link(
        &mut self,
        conversation: ConversationId,
        message: String,
        ctx: &egui::Context,
    ) {
        let web_url = self.conversations.get(&conversation).and_then(|c| {
            c.messages
                .iter()
                .flat_map(|m| std::iter::once(m).chain(m.replies.iter()))
                .find(|m| m.id == message)
                .and_then(|m| m.web_url.clone())
        });
        if let Some(link) = message_link(&conversation, &message, web_url.as_deref()) {
            ctx.copy_text(link);
            self.toast(ToastKind::Info, "Link copied");
        }
    }

    pub(super) fn on_pins(&mut self, chat: String, pins: Vec<PinnedMessage>) {
        self.pins.insert(chat, pins);
    }

    /// Fetches a chat's pins when the person opens it.
    pub(super) fn open_pins(&mut self, chat: &str) {
        if chat != NOTES_CHAT {
            self.fetch(Resource::Pins(chat.to_owned()), Trigger::Opened);
        }
    }

    /// Looks at the pins again when new messages say one changed.
    pub(super) fn after_messages_pins(
        &mut self,
        conversation: &ConversationId,
        messages: &[Message],
    ) {
        if let ConversationId::Chat(chat) = conversation
            && messages.iter().any(|m| m.pin_event)
        {
            self.fetch(Resource::Pins(chat.clone()), Trigger::AfterAction);
        }
    }
}

/// A link to a message in Teams: built for chats, Graph's `webUrl` for
/// channel posts, none otherwise.
pub fn message_link(
    conversation: &ConversationId,
    message: &str,
    web_url: Option<&str>,
) -> Option<String> {
    match conversation {
        ConversationId::Chat(chat) => Some(format!(
            "https://teams.microsoft.com/l/message/{chat}/{message}?context=%7B%22contextType%22%3A%22chat%22%7D"
        )),
        ConversationId::Channel { .. } => web_url.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::backend::{Backend, Waker};
    use crate::paths::AppDirs;
    use crate::settings::Settings;

    fn app() -> (App, tokio::sync::mpsc::UnboundedReceiver<Command>) {
        let (backend, commands) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-unused");
        let app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            false,
        );
        (app, commands)
    }

    fn sent(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn pin(id: &str) -> PinnedMessage {
        PinnedMessage {
            id: id.into(),
            message_id: format!("m-{id}"),
            author: "Ada".into(),
            preview: "hi".into(),
            at: 1,
        }
    }

    fn chan() -> ConversationId {
        ConversationId::Channel {
            team: "t".into(),
            channel: "c".into(),
        }
    }

    #[test]
    fn chat_link_is_built() {
        assert_eq!(
            message_link(&ConversationId::Chat("19:a@thread.v2".into()), "1700", None).as_deref(),
            Some(
                "https://teams.microsoft.com/l/message/19:a@thread.v2/1700?context=%7B%22contextType%22%3A%22chat%22%7D"
            )
        );
        assert_eq!(message_link(&chan(), "1", None), None);
    }

    #[test]
    fn channel_link_uses_web_url() {
        assert_eq!(
            message_link(&chan(), "1", Some("https://x/y")).as_deref(),
            Some("https://x/y")
        );
    }

    #[test]
    fn pin_sends_command() {
        let (mut app, mut rx) = app();
        let ctx = egui::Context::default();
        app.apply(
            Action::PinMessage {
                chat: "19:g".into(),
                message: "m1".into(),
            },
            &ctx,
        );
        assert!(sent(&mut rx).iter().any(|c| matches!(
            c,
            Command::PinMessage { chat, message } if chat == "19:g" && message == "m1"
        )));
    }

    #[test]
    fn unpin_removes_optimistically() {
        let (mut app, mut rx) = app();
        let ctx = egui::Context::default();
        app.on_pins("19:g".into(), vec![pin("a"), pin("b")]);
        app.apply(
            Action::UnpinMessage {
                chat: "19:g".into(),
                pin: "a".into(),
            },
            &ctx,
        );
        assert_eq!(app.pins["19:g"], vec![pin("b")]);
        assert!(sent(&mut rx).iter().any(|c| matches!(
            c,
            Command::UnpinMessage { chat, pin } if chat == "19:g" && pin == "a"
        )));
    }

    fn fetched(cmds: &[Command], chat: &str) -> Option<Trigger> {
        cmds.iter().find_map(|c| match c {
            Command::Fetch {
                resource: Resource::Pins(id),
                trigger,
            } if id == chat => Some(*trigger),
            _ => None,
        })
    }

    #[test]
    fn open_fetches_pins_for_chats_not_channels_or_notes() {
        let (mut app, mut rx) = app();
        app.open_pins("19:g");
        let cmds = sent(&mut rx);
        assert_eq!(fetched(&cmds, "19:g"), Some(Trigger::Opened));
        app.open_pins(NOTES_CHAT);
        assert!(sent(&mut rx).is_empty());
        app.open_conversation(chan());
        assert!(!sent(&mut rx).iter().any(|c| matches!(
            c,
            Command::Fetch {
                resource: Resource::Pins(_),
                ..
            }
        )));
    }

    #[test]
    fn pin_event_refetches() {
        let (mut app, mut rx) = app();
        let chat = ConversationId::Chat("19:g".into());
        let plain = Message::default();
        app.after_messages_pins(&chat, std::slice::from_ref(&plain));
        assert!(sent(&mut rx).is_empty());
        let event = Message {
            pin_event: true,
            ..Default::default()
        };
        app.after_messages_pins(&chat, &[plain, event.clone()]);
        assert_eq!(fetched(&sent(&mut rx), "19:g"), Some(Trigger::AfterAction));
        app.after_messages_pins(&chan(), &[event]);
        assert!(sent(&mut rx).is_empty());
    }

    #[test]
    fn copy_link_toasts_copied() {
        let (mut app, _rx) = app();
        let ctx = egui::Context::default();
        let chat = ConversationId::Chat("19:g".into());
        app.copy_message_link(chat, "m1".into(), &ctx);
        assert_eq!(app.toasts.len(), 1);
        assert_eq!(app.toasts[0].text, "Link copied");
        assert!(ctx.output(|o| {
            o.commands.iter().any(|c| {
                matches!(
                    c,
                    egui::OutputCommand::CopyText(t) if t.contains("/l/message/19:g/m1")
                )
            })
        }));
    }

    #[test]
    fn copy_channel_link_needs_web_url() {
        let (mut app, _rx) = app();
        let ctx = egui::Context::default();
        app.conversations
            .entry(chan())
            .or_default()
            .messages
            .push(Message {
                id: "1".into(),
                ..Default::default()
            });
        app.copy_message_link(chan(), "1".into(), &ctx);
        assert!(app.toasts.is_empty());
        app.conversations.get_mut(&chan()).unwrap().messages[0].web_url =
            Some("https://x/y".into());
        app.copy_message_link(chan(), "1".into(), &ctx);
        assert_eq!(app.toasts.len(), 1);
    }
}
