//! Offline sample data: a small fictional company (Contoso, Microsoft's own
//! placeholder name) with chats, channels, meetings and presence, so the
//! interface can be developed, tested and screenshotted without an account.
//! Nothing here touches the network.

use crate::app::{App, Conversation};
use crate::backend::AuthStatus;
use crate::html::{self, RichText};
use crate::model::*;
use crate::plugin::registry::{Capability, Need};

const ME: &str = "u-me";
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

struct Person {
    id: &'static str,
    name: &'static str,
    email: &'static str,
    availability: Availability,
    activity: Option<&'static str>,
}

const PEOPLE: &[Person] = &[
    Person {
        id: ME,
        name: "Alex Rivera",
        email: "alex@contoso.com",
        availability: Availability::Available,
        activity: None,
    },
    Person {
        id: "u-ada",
        name: "Ada Lovelace",
        email: "ada@contoso.com",
        availability: Availability::Available,
        activity: None,
    },
    Person {
        id: "u-grace",
        name: "Grace Hopper",
        email: "grace@contoso.com",
        availability: Availability::Busy,
        activity: Some("InAMeeting"),
    },
    Person {
        id: "u-katherine",
        name: "Katherine Johnson",
        email: "katherine@contoso.com",
        availability: Availability::Away,
        activity: None,
    },
    Person {
        id: "u-dorothy",
        name: "Dorothy Vaughan",
        email: "dorothy@contoso.com",
        availability: Availability::Offline,
        activity: None,
    },
    Person {
        id: "u-margaret",
        name: "Margaret Hamilton",
        email: "margaret@contoso.com",
        availability: Availability::DoNotDisturb,
        activity: Some("Presenting"),
    },
    Person {
        id: "u-alan",
        name: "Alan Turing",
        email: "alan@contoso.com",
        availability: Availability::BeRightBack,
        activity: None,
    },
    Person {
        id: "u-hedy",
        name: "Hedy Lamarr",
        email: "hedy@contoso.com",
        availability: Availability::Available,
        activity: Some("InACall"),
    },
];

/// An Adaptive Card with the elements Chaat draws: text, facts, columns, an
/// image (a placeholder; images are never fetched) and an Open URL action.
const DEMO_CARD: &str = r#"{"type":"AdaptiveCard","version":"1.4","body":[
{"type":"TextBlock","text":"Launch readiness","weight":"Bolder","size":"Medium"},
{"type":"FactSet","facts":[{"title":"Build","value":"0.2 preview"},{"title":"Owner","value":"Grace Hopper"}]},
{"type":"ColumnSet","columns":[
{"type":"Column","items":[{"type":"TextBlock","text":"Tests","isSubtle":true},{"type":"TextBlock","text":"Passing"}]},
{"type":"Column","items":[{"type":"TextBlock","text":"Signing","isSubtle":true},{"type":"TextBlock","text":"Ad hoc"}]}]},
{"type":"Image","url":"https://example.com/chart.png","altText":"Burndown chart"}],
"actions":[{"type":"Action.OpenUrl","title":"Open checklist","url":"https://example.com/checklist"}]}"#;

fn person(id: &str) -> &'static Person {
    PEOPLE.iter().find(|p| p.id == id).unwrap_or(&PEOPLE[0])
}

fn member(id: &str) -> Member {
    let p = person(id);
    Member {
        tenant_id: None,
        user_id: p.id.into(),
        display_name: p.name.into(),
        email: Some(p.email.into()),
    }
}

struct Builder {
    conversation: ConversationId,
    messages: Vec<Message>,
    next: u32,
}

impl Builder {
    fn new(conversation: ConversationId) -> Self {
        Self {
            conversation,
            messages: Vec::new(),
            next: 0,
        }
    }

    fn id(&mut self) -> String {
        self.next += 1;
        format!("{}-{}", self.conversation.key().len(), self.next)
    }

    fn html(&mut self, author: &str, at: i64, body: &str) -> &mut Message {
        let id = self.id();
        let p = person(author);
        self.messages.push(Message {
            id,
            conversation: Some(self.conversation.clone()),
            author_id: Some(p.id.into()),
            author: p.name.into(),
            created: at,
            modified: at,
            body: html::parse(body),
            ..Default::default()
        });
        self.messages.last_mut().expect("just pushed")
    }

    fn text(&mut self, author: &str, at: i64, body: &str) -> &mut Message {
        let html = html::compose(body);
        self.html(author, at, &html)
    }

    fn system(&mut self, at: i64, text: &str) {
        let id = self.id();
        self.messages.push(Message {
            id,
            conversation: Some(self.conversation.clone()),
            created: at,
            modified: at,
            system: true,
            body: RichText::plain(text),
            ..Default::default()
        });
    }

    fn missed_call(&mut self, at: i64) {
        self.system(at, "Missed call");
        let message = self.messages.last_mut().expect("just pushed");
        message.missed_call = true;
        message.call = Some(crate::model::CallEvent {
            ended: true,
            ..Default::default()
        });
    }

    fn quote(&mut self, author: &str, at: i64, quoted_index: usize, body: &str) {
        let quoted = &self.messages[quoted_index];
        let attachment = Attachment {
            id: quoted.id.clone(),
            kind: AttachmentKind::Quote {
                message_id: quoted.id.clone(),
                author: Some(quoted.author.clone()),
                preview: quoted.body.summary(200),
            },
            name: None,
            content_type: Some("messageReference".into()),
            url: None,
            content: None,
        };
        self.text(author, at, body).attachments.push(attachment);
    }

    fn react(&mut self, emoji: &str, users: &[&str]) {
        if let Some(last) = self.messages.last_mut() {
            last.reactions.push(Reaction {
                emoji: emoji.into(),
                users: users.iter().map(|u| (*u).to_owned()).collect(),
            });
        }
    }

    fn finish(self) -> (ConversationId, Conversation) {
        let mut conversation = Conversation {
            messages: self.messages,
            more: false,
            fresh: true,
            ..Default::default()
        };
        conversation.messages.sort_by_key(|m| m.created);
        (self.conversation, conversation)
    }
}

fn chat(
    id: &str,
    kind: ChatKind,
    topic: Option<&str>,
    members: &[&str],
    last_read_offset: Option<i64>,
) -> Chat {
    Chat {
        id: id.into(),
        kind,
        topic: topic.map(Into::into),
        members: members.iter().map(|m| member(m)).collect(),
        web_url: Some(format!(
            "https://teams.microsoft.com/l/chat/{}/0",
            urlencoding::encode(id)
        )),
        last_read: last_read_offset,
        ..Default::default()
    }
}

/// Fills `app` with the sample company, as of now.
pub fn populate(app: &mut App) {
    let now = crate::time::now();
    let today = crate::time::start_of_day(now);
    app.auth = AuthStatus::SignedIn {
        account: "u-me.demo".into(),
        name: "Alex Rivera".into(),
        username: "alex@contoso.com".into(),
    };
    app.profile = Some(Profile {
        id: ME.into(),
        display_name: "Alex Rivera".into(),
        mail: Some("alex@contoso.com".into()),
        user_principal_name: "alex@contoso.com".into(),
        job_title: Some("Desktop engineer".into()),
        tenant_id: Some("demo".into()),
    });
    app.id = Some("u-me.demo".into());
    app.granted = crate::auth::scope_string(
        &crate::auth::OPTIONAL_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
    );
    app.my_presence = Some(Presence {
        availability: Availability::Available,
        activity: None,
        status_message: None,
    });
    app.channel_access = Access::Granted;
    app.out_of_office = Some(Default::default());

    let mut conversations = Vec::new();
    let mut chats = Vec::new();

    // Ada: the launch plan, with a quote reply, code and reactions.
    {
        let id = "19:demo-ada@unq.gbl.spaces";
        let mut b = Builder::new(ConversationId::Chat(id.into()));
        let t = now - 3 * HOUR;
        b.text(
            "u-ada",
            t - DAY,
            "Morning! Did the Graph throttling numbers come back?",
        );
        b.text(ME, t - DAY + 4 * MINUTE, "They did. 20 requests/s per app per tenant for chat messages, 1 per chat. We fetch on open and on focus, never in a loop.");
        b.react("👍", &["u-ada"]);
        b.text("u-ada", t, "Can we ship the **0.2 preview** on Friday?");
        b.text(ME, t + 2 * MINUTE, "I think so. Remaining items:\n- virtualized history for 10k+ messages\n- channel replies inline\n- reconnect the cache on account switch");
        b.quote(
            "u-ada",
            t + 5 * MINUTE,
            3,
            "Perfect. Let's keep the scope exactly that.",
        );
        b.react("❤️", &[ME]);
        b.text(ME, t + 9 * MINUTE, "Startup is now measured from launch to the first frame. To see it yourself:\n```\nRUST_LOG=chaat=info chaat --measure-startup\n```");
        b.react("🎉", &["u-ada", ME]);
        b.text("u-ada", now - 14 * MINUTE, "Amazing. I'll write the release notes; can you check https://github.com/crmne/spotifast for how they phrase theirs?");
        let preview_at = now - 14 * MINUTE;
        let mut c = chat(
            id,
            ChatKind::OneOnOne,
            None,
            &[ME, "u-ada"],
            // The chat the demo opens on, so it has been read.
            Some(now),
        );
        c.updated = preview_at;
        chats.push(c);
        conversations.push(b.finish());
    }

    // Launch crew: a group with a system event, a mention and links.
    {
        let id = "19:demo-launch@thread.v2";
        let mut b = Builder::new(ConversationId::Chat(id.into()));
        let t = now - 2 * DAY;
        b.system(t, "Ada Lovelace added Grace Hopper and Margaret Hamilton");
        let welcome = b
            .text(
                "u-ada",
                t + MINUTE,
                "Welcome! This chat is for the launch checklist.",
            )
            .id
            .clone();
        let rollback = b.text("u-grace", t + 30 * MINUTE, "I've drafted the rollback plan. Short version: the cache is only a cache, so a bad build can always be rolled back without losing anything.").id.clone();
        b.react("👍", &["u-ada", "u-margaret", ME]);
        b.html("u-margaret", now - 5 * HOUR, "<p><at id=\"0\">Alex Rivera</at> can you confirm the macOS bundle is signed for the preview?</p>");
        b.text(ME, now - 5 * HOUR + 6 * MINUTE, "Ad-hoc signed for now; Developer ID signing needs the team certificate. I'll note it in the release checklist.");
        b.text(
            "u-grace",
            now - 50 * MINUTE,
            "Reminder: ~~Thursday~~ **Wednesday** is the go/no-go.",
        );
        b.react("😮", &["u-ada"]);
        b.text("u-grace", now - 40 * MINUTE, "Go/no-go status:")
            .attachments
            .push(Attachment {
                id: "demo-card".into(),
                kind: AttachmentKind::Card,
                name: None,
                content_type: Some("application/vnd.microsoft.card.adaptive".into()),
                url: None,
                content: Some(DEMO_CARD.into()),
            });
        b.text(
            "u-katherine",
            now - 31 * MINUTE,
            "The throttling review is in the Engineering › Incidents channel.",
        );
        // Two pinned messages; pins are chat-only in Graph.
        app.pins.insert(
            id.into(),
            vec![
                PinnedMessage {
                    id: "demo-pin-1".into(),
                    message_id: rollback,
                    author: "Grace Hopper".into(),
                    preview:
                        "I've drafted the rollback plan. Short version: the cache is only a cache"
                            .into(),
                    at: t + 30 * MINUTE,
                },
                PinnedMessage {
                    id: "demo-pin-2".into(),
                    message_id: welcome,
                    author: "Ada Lovelace".into(),
                    preview: "Welcome! This chat is for the launch checklist.".into(),
                    at: t + MINUTE,
                },
            ],
        );
        let mut c = chat(
            id,
            ChatKind::Group,
            Some("Launch crew"),
            &[ME, "u-ada", "u-grace", "u-margaret", "u-katherine"],
            Some(now - 2 * HOUR),
        );
        c.updated = now - 31 * MINUTE;
        chats.push(c);
        conversations.push(b.finish());
    }

    // Grace: an edited message.
    {
        let id = "19:demo-grace@unq.gbl.spaces";
        let mut b = Builder::new(ConversationId::Chat(id.into()));
        b.text(
            "u-grace",
            now - DAY - 3 * HOUR,
            "Found the bug. It was a literal moth in relay #70.",
        );
        b.react("😆", &[ME]);
        b.text(
            ME,
            now - DAY - 3 * HOUR + MINUTE,
            "Tape it into the log book 😄",
        );
        b.missed_call(now - DAY - 2 * HOUR - 10 * MINUTE);
        b.text("u-grace", now - DAY - 2 * HOUR, "Done. Also: the HTML parser handled every message in the corpus, including the weird Outlook ones.").edited = true;
        let mut c = chat(id, ChatKind::OneOnOne, None, &[ME, "u-grace"], Some(now));
        c.updated = now - DAY - 2 * HOUR;
        chats.push(c);
        conversations.push(b.finish());
    }

    // A meeting chat with its join link.
    {
        let id = "19:meeting_demo-design-review@thread.v2";
        let mut b = Builder::new(ConversationId::Chat(id.into()));
        b.system(now - 26 * HOUR, "Meeting started");
        b.text(
            "u-margaret",
            now - 26 * HOUR + 3 * MINUTE,
            "Agenda: sidebar density, keyboard navigation, light theme contrast.",
        );
        b.text(
            "u-hedy",
            now - 26 * HOUR + 20 * MINUTE,
            "The quick switcher (⌘K) is the best part. Can it also search messages?",
        );
        b.text(
            ME,
            now - 26 * HOUR + 22 * MINUTE,
            "It does: local full-text search over the cache, then Microsoft Search for the rest.",
        );
        b.system(now - 25 * HOUR, "Meeting ended after 58m");
        // Graph posts an event while the recording runs (no link), then
        // the one with the link; the view hides the first.
        b.system(now - 25 * HOUR + MINUTE / 2, "Call recorded");
        b.messages.last_mut().expect("just pushed").recording_event = true;
        b.system(now - 25 * HOUR + MINUTE, "Call recorded");
        let recorded = b.messages.last_mut().expect("just pushed");
        recorded.recording_event = true;
        recorded.recording = Some(crate::model::Recording {
            url: "https://contoso-my.sharepoint.com/personal/demo/Recordings/design-review.mp4"
                .into(),
            name: "Design review".into(),
            duration: Some("58m".into()),
        });
        let mut c = chat(
            id,
            ChatKind::Meeting,
            Some("Design review: Chaat 0.2"),
            &[ME, "u-margaret", "u-hedy", "u-ada"],
            Some(now),
        );
        c.join_url = Some("https://teams.microsoft.com/l/meetup-join/19%3ameeting_demo-design-review%40thread.v2/0".into());
        c.updated = now - 25 * HOUR;
        chats.push(c);
        conversations.push(b.finish());
    }

    // Katherine: unread.
    {
        let id = "19:demo-katherine@unq.gbl.spaces";
        let mut b = Builder::new(ConversationId::Chat(id.into()));
        b.text(
            "u-katherine",
            now - 4 * DAY,
            "Checked the trajectory math for the scroll physics. It's right.",
        );
        b.text(ME, now - 4 * DAY + 10 * MINUTE, "Thank you! Ship it?");
        b.text("u-katherine", now - 7 * MINUTE, "One more thing: the unread divider jumps by one row when a reply arrives while you're scrolled up.");
        let mut c = chat(
            id,
            ChatKind::OneOnOne,
            None,
            &[ME, "u-katherine"],
            Some(now - 4 * DAY),
        );
        c.updated = now - 7 * MINUTE;
        chats.push(c);
        conversations.push(b.finish());
    }

    // On-call: unread, urgent.
    {
        let id = "19:demo-oncall@thread.v2";
        let mut b = Builder::new(ConversationId::Chat(id.into()));
        b.text("u-alan", now - 3 * DAY, "Handing over on-call. Quiet week.");
        b.text("u-hedy", now - 2 * MINUTE, "Token refresh failures spiking for one tenant (AADSTS50173). Probably a password reset wave; Chaat asks those users to sign in again.").importance_high = true;
        let mut c = chat(
            id,
            ChatKind::Group,
            Some("On-call"),
            &[ME, "u-alan", "u-hedy", "u-dorothy"],
            Some(now - 3 * DAY),
        );
        c.updated = now - 2 * MINUTE;
        chats.push(c);
        conversations.push(b.finish());
    }

    // A chat the person hid; it shows under the Hidden chip.
    {
        let id = "19:demo-offsite@thread.v2";
        let mut b = Builder::new(ConversationId::Chat(id.into()));
        let at = now - 9 * DAY;
        b.text(
            "u-ada",
            at,
            "Offsite venue shortlist is in the planning doc.",
        );
        let mut c = chat(
            id,
            ChatKind::Group,
            Some("Offsite planning"),
            &[ME, "u-ada", "u-dorothy"],
            Some(now),
        );
        c.updated = at;
        c.hidden = true;
        chats.push(c);
        conversations.push(b.finish());
    }

    // Older one-on-ones.
    for (index, (id, other, text)) in [
        (
            "19:demo-dorothy@unq.gbl.spaces",
            "u-dorothy",
            "The FORTRAN course notes are in the shared drive.",
        ),
        (
            "19:demo-margaret@unq.gbl.spaces",
            "u-margaret",
            "Priority displays saved the landing. Let's do the same for error toasts.",
        ),
        (
            "19:demo-alan@unq.gbl.spaces",
            "u-alan",
            "Is the switcher's fuzzy match decidable? Asking for a friend.",
        ),
        (
            "19:demo-hedy@unq.gbl.spaces",
            "u-hedy",
            "Frequency hopping for the reconnect backoff, maybe?",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut b = Builder::new(ConversationId::Chat(id.into()));
        let at = now - (index as i64 + 5) * DAY - 2 * HOUR;
        b.text(other, at, text);
        b.text(ME, at + 15 * MINUTE, "Noted, thanks!");
        let mut c = chat(id, ChatKind::OneOnOne, None, &[ME, other], Some(now));
        c.updated = at + 15 * MINUTE;
        chats.push(c);
        conversations.push(b.finish());
    }

    // Teams and channels.
    let teams = vec![
        team("t-eng", "Engineering", "Builds and runs Chaat"),
        team("t-product", "Product", "Roadmap, design and research"),
        team("t-all", "Contoso", "Company-wide news"),
    ];
    let channels = [
        (
            "t-eng",
            "19:eng-general@thread.tacv2",
            "General",
            ChannelKind::Standard,
        ),
        (
            "t-eng",
            "19:eng-desktop@thread.tacv2",
            "Desktop clients",
            ChannelKind::Standard,
        ),
        (
            "t-eng",
            "19:eng-backend@thread.tacv2",
            "Graph integration",
            ChannelKind::Standard,
        ),
        (
            "t-eng",
            "19:eng-incidents@thread.tacv2",
            "Incidents",
            ChannelKind::Private,
        ),
        (
            "t-product",
            "19:product-general@thread.tacv2",
            "General",
            ChannelKind::Standard,
        ),
        (
            "t-product",
            "19:product-design@thread.tacv2",
            "Design",
            ChannelKind::Standard,
        ),
        (
            "t-product",
            "19:product-research@thread.tacv2",
            "Research",
            ChannelKind::Shared,
        ),
        (
            "t-all",
            "19:all-general@thread.tacv2",
            "General",
            ChannelKind::Standard,
        ),
    ];
    for (team_id, id, name, kind) in channels {
        app.channels
            .entry(team_id.into())
            .or_default()
            .push(Channel {
                id: id.into(),
                team_id: team_id.into(),
                name: name.into(),
                description: None,
                kind,
                web_url: Some(format!(
                    "https://teams.microsoft.com/l/channel/{}/{}",
                    urlencoding::encode(id),
                    urlencoding::encode(name)
                )),
            });
    }

    // Desktop clients: threads with replies.
    {
        let mut b = Builder::new(ConversationId::Channel {
            team: "t-eng".into(),
            channel: "19:eng-desktop@thread.tacv2".into(),
        });
        let root = b.text("u-margaret", now - 2 * DAY, "Proposal: draw message bodies natively from Teams HTML instead of embedding a web view. Parser handles p, b/i/u/s, links, code, quotes, lists, mentions, emoji and inline images.");
        root.subject = Some("No browser engine in the client".into());
        let root_id = root.id.clone();
        b.react("👍", &["u-ada", "u-grace", ME]);
        let replies = vec![
            reply(
                &root_id,
                "u-grace",
                now - 2 * DAY + 20 * MINUTE,
                "+1. Anything unknown degrades to text, never to a crash.",
            ),
            reply(
                &root_id,
                ME,
                now - 2 * DAY + 45 * MINUTE,
                "Fuzzed with malformed input; the tokenizer never panics. Tests are in src/html.rs.",
            ),
        ];
        if let Some(m) = b.messages.last_mut() {
            m.replies = replies;
        }
        let root = b.text("u-ada", now - 6 * HOUR, "Idle should cost nothing: egui only repaints on input or backend events, and nothing polls.");
        root.subject = Some("Performance check".into());
        let root_id = root.id.clone();
        b.react("🎉", &["u-margaret", "u-hedy"]);
        let replies = vec![
            reply(&root_id, "u-hedy", now - 5 * HOUR, "And memory?"),
            reply(
                &root_id,
                "u-ada",
                now - 5 * HOUR + 3 * MINUTE,
                "Same doc: resident memory with the demo data loaded, measured on each platform.",
            ),
        ];
        if let Some(m) = b.messages.last_mut() {
            m.replies = replies;
        }
        b.text("u-hedy", now - 40 * MINUTE, "Calling: Graph can't place calls as a user, so Chaat opens Teams through the documented deep links. Details in docs/calling.md.");
        conversations.push(b.finish());
    }
    {
        let mut b = Builder::new(ConversationId::Channel {
            team: "t-eng".into(),
            channel: "19:eng-general@thread.tacv2".into(),
        });
        let root = b.text("u-alan", now - 3 * DAY, "Welcome to Engineering. Please keep build questions in Desktop clients and API questions in Graph integration.");
        root.subject = Some("Read me first".into());
        b.text(
            "u-dorothy",
            now - DAY,
            "New laptops arrive Monday. Install Chaat from the internal share.",
        );
        conversations.push(b.finish());
    }
    {
        let mut b = Builder::new(ConversationId::Channel {
            team: "t-eng".into(),
            channel: "19:eng-incidents@thread.tacv2".into(),
        });
        let root = b.text("u-katherine", now - 35 * MINUTE, "Throttling review: with fetch-on-open there is no background traffic at all, so an idle client sends no requests.");
        root.subject = Some("Throttling review".into());
        root.importance_high = true;
        conversations.push(b.finish());
    }
    {
        let mut b = Builder::new(ConversationId::Channel {
            team: "t-product".into(),
            channel: "19:product-design@thread.tacv2".into(),
        });
        let root = b.text("u-margaret", now - DAY, "Design principles for Chaat:\n1. The conversation is the workspace\n2. Every view opens from cache instantly\n3. Dense, calm, readable");
        root.subject = Some("Principles".into());
        b.react("❤️", &[ME, "u-hedy"]);
        conversations.push(b.finish());
    }

    // Calendar.
    let meetings = vec![
        meeting(
            "m1",
            "Standup",
            today + 9 * HOUR + 30 * MINUTE,
            15,
            Some("Teams"),
            "Grace Hopper",
            true,
        ),
        meeting(
            "demo-design-review",
            "Design review: Chaat 0.2",
            now + 25 * MINUTE,
            45,
            Some("Teams"),
            "Margaret Hamilton",
            true,
        ),
        meeting(
            "m3",
            "Focus time",
            today + 14 * HOUR,
            120,
            None,
            "Alex Rivera",
            false,
        ),
        meeting(
            "m4",
            "Release go/no-go",
            today + DAY + 11 * HOUR,
            30,
            Some("Room 4 + Teams"),
            "Ada Lovelace",
            true,
        ),
        meeting(
            "m5",
            "1:1 Ada / Alex",
            today + DAY + 15 * HOUR,
            30,
            Some("Teams"),
            "Ada Lovelace",
            true,
        ),
        meeting(
            "m6",
            "Contoso all hands",
            today + 3 * DAY + 16 * HOUR,
            60,
            Some("Auditorium"),
            "Dorothy Vaughan",
            true,
        ),
    ];

    for p in PEOPLE {
        app.presence.insert(
            p.id.into(),
            Presence {
                availability: p.availability,
                activity: p.activity.map(Into::into),
                status_message: (p.id == "u-katherine").then(|| "Back at 3 pm".into()),
            },
        );
    }

    // Each chat's preview from its newest message.
    for chat in &mut chats {
        let key = ConversationId::Chat(chat.id.clone());
        if let Some((_, conversation)) = conversations.iter().find(|(id, _)| *id == key)
            && let Some(last) = conversation
                .messages
                .iter()
                .rev()
                .find(|m| !m.system)
                .or(conversation.messages.last())
        {
            chat.preview = Some(Preview {
                message_id: last.id.clone(),
                author_id: last.author_id.clone(),
                author: Some(last.author.clone()),
                text: last.body.summary(140),
                at: last.created,
                deleted: false,
                ..Default::default()
            });
        }
    }
    chats.sort_by_key(|chat| std::cmp::Reverse(chat.activity()));

    app.expanded_teams.insert("t-eng".into());
    app.expanded_teams.insert("t-product".into());
    app.teams = teams;
    app.chats = chats;
    app.meetings = meetings;
    app.conversations = conversations.into_iter().collect();
    app.open = Some(ConversationId::Chat("19:demo-ada@unq.gbl.spaces".into()));
    app.section = Section::Chats;

    // A second account, so the account switcher has something to switch to.
    let shown = app.active;
    app.accounts.push(crate::app::Account::new(
        crate::backend::Backend::detached().0,
    ));
    app.active = app.accounts.len() - 1;
    populate_second(app, now);
    app.active = shown;
    populate_plugins(app);
}

/// Two demo plugins for the Plugins page: a call-alerts plugin that listens
/// for the first account and waits for a sign-in on the second, and a
/// presence light with one option of every kind.
fn populate_plugins(app: &mut App) {
    use crate::plugin::protocol::{PluginInfo, StatusState};
    use crate::settings::PluginEntry;

    let accounts = app.plugin_accounts();
    app.settings.plugins = vec![
        PluginEntry {
            id: "demo-alerts".into(),
            path: "plugins/call-alerts.exe".into(),
            enabled: true,
            needs: vec!["accounts".into()],
            approved_needs: vec!["accounts".into()],
            ..Default::default()
        },
        PluginEntry {
            id: "demo-light".into(),
            path: "plugins/status-light.exe".into(),
            enabled: true,
            needs: vec!["presence".into()],
            options: [
                ("flash".to_owned(), serde_json::json!(true)),
                ("ip".to_owned(), serde_json::json!("192.168.1.20")),
                ("busy".to_owned(), serde_json::json!("#e5484d")),
                ("level".to_owned(), serde_json::json!(80)),
                // The plugin no longer offers this effect.
                ("mode".to_owned(), serde_json::json!("pulse")),
            ]
            .into(),
            secrets: vec!["token".into()],
            ..Default::default()
        },
        // Its file changed since it was added; it did not run.
        PluginEntry {
            id: "demo-changed".into(),
            path: "plugins/desk-lamp.exe".into(),
            enabled: true,
            ..Default::default()
        },
    ];
    let alerts = app.plugins.entry("demo-alerts".into()).or_default();
    alerts.running = true;
    alerts.info = PluginInfo {
        name: Some("Ring alerts".into()),
        description: Some("Tells Chaat who is calling you in Teams.".into()),
        docs_url: Some("https://example.com/call-alerts".into()),
    };
    alerts.offers = vec![Capability::CallAlerts];
    alerts.needs = vec![Need::Accounts];
    alerts.listening = accounts.first().map(|a| a.id.clone()).into_iter().collect();
    if let Some(second) = accounts.get(1) {
        alerts.sign_in.push(PluginSignIn {
            plugin: "demo-alerts".into(),
            account: Some(second.id.clone()),
            username: Some(second.username.clone()),
            verification_uri: "https://microsoft.com/devicelogin".into(),
            user_code: "DEMO1234".into(),
        });
    }
    let light = app.plugins.entry("demo-light".into()).or_default();
    light.running = true;
    light.info = PluginInfo {
        name: Some("Status light".into()),
        description: Some("Shows your Teams presence on a bulb.".into()),
        docs_url: None,
    };
    light.offers = vec![Capability::PresenceDisplay];
    light.needs = vec![Need::Presence];
    // Asks for presence; the person has not answered yet.
    light.pending_needs = vec!["presence"];
    light.options = demo_light_options();
    // What the plugin last said about its buttons and one setting.
    let now = std::time::Instant::now();
    light.statuses.insert(
        "scan".into(),
        (StatusState::Working, Some("Searching".into()), now),
    );
    light.statuses.insert(
        "test".into(),
        (StatusState::Ok, Some("The light turned on".into()), now),
    );
    light.statuses.insert(
        "ip".into(),
        (
            StatusState::Error,
            Some("No answer from the bulb".into()),
            now,
        ),
    );
    // A value the person typed that Chaat refused.
    app.plugin_option_errors.insert(
        ("demo-light".into(), "level".into()),
        "Enter a number from 10 to 100.".into(),
    );
    let lamp = app.plugins.entry("demo-changed".into()).or_default();
    lamp.info = PluginInfo {
        name: Some("Desk lamp".into()),
        description: None,
        docs_url: None,
    };
    lamp.file_changed =
        Some("3f9a1c07be52d8e4a6b0c9d17f2e8a45b3c6d9e01f7a2b48c5d0e63f91a7b2cd".into());
}

/// The settings of the demo presence light: every kind and field of the
/// option schema.
fn demo_light_options() -> Vec<crate::plugin::protocol::OptionSpec> {
    use crate::plugin::protocol::{OptionChoice, OptionKind, OptionSpec, TextFormat, VisibleWhen};
    use serde_json::json;

    let spec = |id: &str, kind: OptionKind, label: &str| OptionSpec {
        id: id.into(),
        kind,
        label: label.into(),
        ..OptionSpec::default()
    };
    let text = |value: &str| Some(value.to_owned());
    let choice = |value: &str, label: &str| OptionChoice {
        value: value.into(),
        label: label.into(),
    };
    let bulb = text("Bulb");
    let colors = text("Colors");
    vec![
        OptionSpec {
            section: bulb.clone(),
            help: text("Looks for bulbs on your network."),
            ..spec("scan", OptionKind::Action, "Find bulbs")
        },
        OptionSpec {
            section: bulb.clone(),
            required: true,
            format: Some(TextFormat::Ip),
            placeholder: text("192.168.1.20"),
            help: text("The bulb's address on your network."),
            ..spec("ip", OptionKind::Text, "Bulb address")
        },
        OptionSpec {
            section: bulb.clone(),
            required: true,
            help: text("Kept in your system's credential store."),
            ..spec("token", OptionKind::Secret, "Access token")
        },
        OptionSpec {
            section: bulb.clone(),
            ..spec("test", OptionKind::Action, "Test the light")
        },
        OptionSpec {
            section: bulb.clone(),
            default: Some(json!(false)),
            help: text("Blinks the bulb when a call rings."),
            ..spec("flash", OptionKind::Toggle, "Flash on calls")
        },
        OptionSpec {
            section: bulb,
            advanced: true,
            placeholder: text("Optional"),
            help: text("Picks the bulb when more than one answers."),
            ..spec("device_id", OptionKind::Text, "Device ID")
        },
        OptionSpec {
            section: colors.clone(),
            default: Some(json!("high")),
            choices: vec![choice("low", "Low"), choice("high", "High")],
            ..spec("brightness", OptionKind::Choice, "Brightness")
        },
        OptionSpec {
            section: colors.clone(),
            default: Some(json!("steady")),
            choices: vec![choice("steady", "Steady"), choice("blink", "Blink")],
            ..spec("mode", OptionKind::Choice, "Effect")
        },
        OptionSpec {
            section: colors.clone(),
            default: Some(json!("#e5484d")),
            ..spec("busy", OptionKind::Color, "Busy color")
        },
        OptionSpec {
            section: colors.clone(),
            default: Some(json!("#f5a524")),
            ..spec("away", OptionKind::Color, "Away color")
        },
        OptionSpec {
            section: colors.clone(),
            default: Some(json!("#ffffff")),
            visible_when: Some(VisibleWhen {
                option: "flash".into(),
                equals: json!(true),
            }),
            help: text("Shown only while flashing on calls is on."),
            ..spec("flash_color", OptionKind::Color, "Flash color")
        },
        OptionSpec {
            section: colors.clone(),
            default: Some(json!(80)),
            min: Some(10.0),
            max: Some(100.0),
            step: Some(5.0),
            unit: text("%"),
            placeholder: text("10 to 100"),
            ..spec("level", OptionKind::Number, "Dim level")
        },
        OptionSpec {
            section: colors.clone(),
            placeholder: text("Anything to remember"),
            max_length: Some(500),
            ..spec("notes", OptionKind::Multiline, "Notes")
        },
        OptionSpec {
            section: colors,
            advanced: true,
            default: Some(json!(false)),
            ..spec("quit_off", OptionKind::Toggle, "Turn off when Chaat quits")
        },
        spec(
            "wheel",
            OptionKind::Unsupported {
                kind: "dial".into(),
            },
            "Color wheel",
        ),
    ]
}

/// People in the demo directory who are in no chat yet, for "New chat".
const DIRECTORY: &[(&str, &str, &str)] = &[
    ("u-barbara", "Barbara Liskov", "barbara@contoso.com"),
    ("u-radia", "Radia Perlman", "radia@contoso.com"),
    ("u-frances", "Frances Allen", "frances@contoso.com"),
    ("u-annie", "Annie Easley", "annie@contoso.com"),
];

/// The demo's directory search: everyone in the sample company.
pub fn search_people(app: &App, query: &str) -> Vec<Member> {
    let needle = query.trim().to_lowercase();
    let mut people: Vec<Member> = PEOPLE
        .iter()
        .filter(|p| p.id != ME)
        .map(|p| member(p.id))
        .chain(DIRECTORY.iter().map(|(id, name, email)| Member {
            tenant_id: None,
            user_id: (*id).into(),
            display_name: (*name).into(),
            email: Some((*email).into()),
        }))
        .filter(|m| {
            m.display_name.to_lowercase().contains(&needle)
                || m.email.as_deref().is_some_and(|e| e.contains(&needle))
        })
        .collect();
    people.retain(|p| p.user_id != app.me());
    people
}

/// A chat the demo makes on the spot.
pub fn new_chat(app: &App, others: Vec<Member>, topic: Option<String>) -> Chat {
    let mut members = vec![member(ME)];
    members.extend(others);
    Chat {
        id: format!("19:demo-new-{}@thread.v2", app.chats.len()),
        kind: if members.len() == 2 {
            ChatKind::OneOnOne
        } else {
            ChatKind::Group
        },
        topic: topic.filter(|t| !t.trim().is_empty() && members.len() > 2),
        members,
        updated: crate::time::now(),
        ..Default::default()
    }
}

/// Alex's second job: a smaller Fabrikam account.
fn populate_second(app: &mut App, now: i64) {
    app.id = Some("u-alex2.fabrikam".into());
    app.auth = AuthStatus::SignedIn {
        account: "u-alex2.fabrikam".into(),
        name: "Alex Rivera".into(),
        username: "alex@fabrikam.com".into(),
    };
    app.profile = Some(Profile {
        id: "u-alex2".into(),
        display_name: "Alex Rivera".into(),
        mail: Some("alex@fabrikam.com".into()),
        user_principal_name: "alex@fabrikam.com".into(),
        job_title: Some("Advisor".into()),
        tenant_id: Some("fabrikam".into()),
    });
    app.granted = crate::auth::scope_string(&[]);
    let advisor = |id: &str, name: &str, email: &str| Member {
        tenant_id: None,
        user_id: id.into(),
        display_name: name.into(),
        email: Some(email.into()),
    };
    let me = advisor("u-alex2", "Alex Rivera", "alex@fabrikam.com");
    let mae = advisor("u-mae", "Mae Jemison", "mae@fabrikam.com");
    let evelyn = advisor("u-evelyn", "Evelyn Boyd Granville", "evelyn@fabrikam.com");
    let chat_id = "19:fabrikam-mae@unq.gbl.spaces";
    let message = |id: &str, author: &Member, at: i64, text: &str| Message {
        id: id.into(),
        conversation: Some(ConversationId::Chat(chat_id.into())),
        author_id: Some(author.user_id.clone()),
        author: author.display_name.clone(),
        created: at,
        modified: at,
        body: html::parse(&html::compose(text)),
        ..Default::default()
    };
    let messages = vec![
        message(
            "f1",
            &mae,
            now - 2 * DAY,
            "Thanks for the advisory call. Could you review the onboarding flow?",
        ),
        message(
            "f2",
            &me,
            now - 2 * DAY + HOUR,
            "Happy to. I'll send notes by Friday.",
        ),
        message(
            "f3",
            &mae,
            now - 20 * MINUTE,
            "No rush. Also: are you joining the board meeting on Thursday?",
        ),
    ];
    app.chats = vec![
        Chat {
            id: chat_id.into(),
            kind: ChatKind::OneOnOne,
            members: vec![me.clone(), mae.clone()],
            preview: Some(Preview {
                message_id: "f3".into(),
                author_id: Some("u-mae".into()),
                author: Some("Mae Jemison".into()),
                text: "No rush. Also: are you joining the board meeting on Thursday?".into(),
                at: now - 20 * MINUTE,
                deleted: false,
                ..Default::default()
            }),
            last_read: Some(now - DAY),
            updated: now - 20 * MINUTE,
            ..Default::default()
        },
        Chat {
            id: "19:fabrikam-board@thread.v2".into(),
            kind: ChatKind::Group,
            topic: Some("Fabrikam advisors".into()),
            members: vec![me, mae, evelyn],
            preview: Some(Preview {
                message_id: "g1".into(),
                author_id: Some("u-evelyn".into()),
                author: Some("Evelyn Boyd Granville".into()),
                text: "Deck for Thursday is in the shared folder.".into(),
                at: now - 3 * DAY,
                deleted: false,
                ..Default::default()
            }),
            last_read: Some(now),
            updated: now - 3 * DAY,
            ..Default::default()
        },
    ];
    let mut conversation = Conversation {
        fresh: true,
        ..Default::default()
    };
    conversation.messages = messages;
    app.conversations = [(ConversationId::Chat(chat_id.into()), conversation)]
        .into_iter()
        .collect();
    app.presence.insert(
        "u-mae".into(),
        Presence {
            availability: Availability::Available,
            ..Default::default()
        },
    );
    app.presence.insert(
        "u-evelyn".into(),
        Presence {
            availability: Availability::Away,
            ..Default::default()
        },
    );
    app.my_presence = Some(Presence {
        availability: Availability::Available,
        ..Default::default()
    });
    app.out_of_office = Some(Default::default());
    app.presence.insert(
        "u-alex2".into(),
        Presence {
            availability: Availability::Available,
            ..Default::default()
        },
    );
    app.open = Some(ConversationId::Chat(chat_id.into()));
}

fn reply(root: &str, author: &str, at: i64, text: &str) -> Message {
    let p = person(author);
    Message {
        id: format!("{root}-r{at}"),
        reply_to: Some(root.into()),
        author_id: Some(p.id.into()),
        author: p.name.into(),
        created: at,
        modified: at,
        body: html::parse(&html::compose(text)),
        ..Default::default()
    }
}

fn team(id: &str, name: &str, description: &str) -> Team {
    Team {
        id: id.into(),
        name: name.into(),
        description: Some(description.into()),
        web_url: None,
    }
}

fn meeting(
    id: &str,
    subject: &str,
    start: i64,
    minutes: i64,
    location: Option<&str>,
    organizer: &str,
    online: bool,
) -> Meeting {
    Meeting {
        id: id.into(),
        subject: subject.into(),
        start,
        end: start + minutes * MINUTE,
        all_day: false,
        location: location.map(Into::into),
        organizer: Some(organizer.into()),
        join_url: online.then(|| {
            format!("https://teams.microsoft.com/l/meetup-join/19%3ameeting_{id}%40thread.v2/0")
        }),
        web_link: None,
        cancelled: false,
        preview: online.then(|| "Join on your computer, mobile app or room device.".into()),
    }
}

/// The demo chat the agent samples sit in.
pub const AGENTS_CHAT: &str = "19:demo-ada@unq.gbl.spaces";

/// For tests: an agents plugin that is on, plus one proposed reply (to Ada's
/// first message) and one proposed reaction (on her last) in the first demo
/// chat. The normal demo has none of this.
pub fn populate_agents(app: &mut App) {
    use crate::settings::PluginEntry;

    app.settings.plugins.push(PluginEntry {
        id: "demo-agents".into(),
        path: "plugins/agents.exe".into(),
        enabled: true,
        needs: vec!["messages".into()],
        approved_needs: vec!["messages".into()],
        ..Default::default()
    });
    let state = app.plugins.entry("demo-agents".into()).or_default();
    state.running = true;
    state.info.name = Some("Agents".into());
    state.offers = vec![Capability::Agents];
    state.needs = vec![Need::Messages];

    let account = app.id.clone().unwrap_or_default();
    let conversation = ConversationId::Chat(AGENTS_CHAT.into());
    let (first, last) = {
        let messages = &app.conversation(&conversation).expect("demo chat").messages;
        let from_ada = |m: &&Message| m.author_id.as_deref() == Some("u-ada");
        (
            messages.iter().find(from_ada).map(|m| m.id.clone()),
            messages.iter().rev().find(from_ada).map(|m| m.id.clone()),
        )
    };
    let now = crate::time::now();
    let agent = "Claude Code".to_owned();
    app.agent_proposals.reply(
        "demo-agents",
        &account,
        conversation.clone(),
        "Thanks Ada. I will check how Spotifast phrases its notes and send you a draft this afternoon."
            .into(),
        first,
        agent.clone(),
        now - 2 * MINUTE,
    );
    let _ = app.agent_proposals.reaction(
        "demo-agents",
        &account,
        conversation,
        last.expect("an Ada message"),
        "👍".into(),
        agent,
        now - MINUTE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_meeting_chat_has_a_recording() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            crate::paths::AppDirs::under(dir.path()),
            crate::settings::Settings::default(),
            crate::backend::Waker::default(),
            true,
        );
        let id = ConversationId::Chat("19:meeting_demo-design-review@thread.v2".into());
        let recordings: Vec<_> = app.conversations[&id]
            .messages
            .iter()
            .filter_map(|m| m.recording.as_ref())
            .collect();
        assert_eq!(recordings.len(), 1);
        assert_eq!(recordings[0].name, "Design review");
        assert_eq!(recordings[0].duration.as_deref(), Some("58m"));
    }

    #[test]
    fn demo_data_is_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            crate::paths::AppDirs::under(dir.path()),
            crate::settings::Settings::default(),
            crate::backend::Waker::default(),
            true,
        );
        assert!(app.is_signed_in());
        assert!(app.chats.len() >= 8);
        assert!(app.unread_chats() >= 2);
        for chat in &app.chats {
            assert!(
                app.conversations
                    .contains_key(&ConversationId::Chat(chat.id.clone())),
                "{}",
                chat.id
            );
            assert!(chat.preview.is_some());
        }
        for meeting in &app.meetings {
            if let Some(url) = &meeting.join_url {
                assert!(crate::calling::is_teams_link(url));
            }
        }
        // Ids are unique within each conversation.
        for conversation in app.conversations.values() {
            let mut ids: Vec<&str> = conversation
                .messages
                .iter()
                .map(|m| m.id.as_str())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), conversation.messages.len());
        }
    }
}
