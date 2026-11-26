//! Graph's shapes into Chaat's model.

use super::types as g;
use crate::html;
use crate::model::{
    Attachment, AttachmentKind, AutoReplies, Availability, Channel, ChannelKind, Chat, ChatKind,
    ConversationId, ExternalAudience, Meeting, Member, Message, OutOfOffice, PinnedMessage,
    Presence, Preview, Profile, Reaction, Team, reaction_emoji,
};
use crate::time::{parse_civil, parse_iso};

pub fn profile(user: g::User, tenant_id: Option<String>) -> Profile {
    Profile {
        display_name: user.display_name.unwrap_or_default(),
        user_principal_name: user.user_principal_name.unwrap_or_default(),
        mail: user.mail,
        job_title: user.job_title,
        id: user.id,
        tenant_id,
    }
}

fn body_text(body: Option<&g::ItemBody>) -> html::RichText {
    let Some(body) = body else {
        return html::RichText::default();
    };
    let content = body.content.as_deref().unwrap_or_default();
    match body.content_type.as_deref() {
        Some(kind) if kind.eq_ignore_ascii_case("text") => {
            html::RichText::plain(&without_attachment_tags(content))
        }
        _ => html::parse(content),
    }
}

/// A plain-text body still carries Teams' `<attachment id="…"></attachment>`
/// placeholder for each file; the files show as chips, so the tags go.
fn without_attachment_tags(content: &str) -> String {
    const OPEN: &str = "<attachment";
    const CLOSE: &str = "</attachment>";
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find(OPEN) {
        let Some(end) = rest[start..].find(CLOSE) else {
            break;
        };
        out.push_str(&rest[..start]);
        rest = &rest[start + end + CLOSE.len()..];
    }
    out.push_str(rest);
    let trimmed = out.trim_end();
    trimmed.to_owned()
}

pub fn chat(chat: g::Chat, me: &str) -> Chat {
    let kind = match chat.chat_type.as_deref() {
        Some("group") => ChatKind::Group,
        Some("meeting") => ChatKind::Meeting,
        _ => ChatKind::OneOnOne,
    };
    let members = chat
        .members
        .unwrap_or_default()
        .into_iter()
        .filter_map(|member| {
            Some(Member {
                user_id: member.user_id?,
                display_name: member.display_name.unwrap_or_default(),
                email: member.email,
                tenant_id: member.tenant_id,
            })
        })
        .collect();
    let preview = chat.last_message_preview.map(|preview| {
        let deleted = preview.is_deleted.unwrap_or(false);
        let system = !deleted
            && preview
                .message_type
                .as_deref()
                .is_some_and(|t| t != "message");
        let text = if deleted {
            "This message was deleted".to_owned()
        } else if system {
            system_summary(&detail_of(preview.event_detail.as_ref()), me)
        } else {
            body_text(preview.body.as_ref()).summary(140)
        };
        Preview {
            message_id: preview.id,
            author_id: preview
                .from
                .as_ref()
                .and_then(|f| f.id())
                .map(str::to_owned),
            author: preview
                .from
                .as_ref()
                .and_then(|f| f.name())
                .map(str::to_owned),
            text,
            at: preview
                .created_date_time
                .as_deref()
                .and_then(parse_iso)
                .unwrap_or_default(),
            deleted,
            system,
            membership: system
                .then(|| membership(&detail_of(preview.event_detail.as_ref())))
                .flatten(),
        }
    });
    let viewpoint = chat.viewpoint.unwrap_or_default();
    Chat {
        id: chat.id,
        kind,
        topic: chat.topic,
        members,
        preview,
        last_read: viewpoint
            .last_message_read_date_time
            .as_deref()
            .and_then(parse_iso),
        updated: chat
            .last_updated_date_time
            .or(chat.created_date_time)
            .as_deref()
            .and_then(parse_iso)
            .unwrap_or_default(),
        web_url: chat.web_url,
        join_url: chat.online_meeting_info.and_then(|info| info.join_web_url),
        marked_unread: false,
        hidden: viewpoint.is_hidden.unwrap_or(false),
    }
}

/// An `eventDetail` read through [`g::EventDetail`]. Missing, unknown and
/// malformed details are all `Other`.
fn detail_of(detail: Option<&serde_json::Value>) -> g::EventDetail {
    detail.map_or(g::EventDetail::Other, g::EventDetail::from_value)
}

/// Words for a system event, from its `eventDetail` type. `me` is the
/// signed-in person's id, to tell a missed call from one they placed.
fn system_summary(detail: &g::EventDetail, me: &str) -> String {
    use g::EventDetail as D;
    let who = |who: &Option<String>| who.clone().unwrap_or_else(|| "Someone".into());
    if let Some(membership) = membership(detail) {
        return membership.describe(|_| None);
    }
    match detail {
        D::ChatRenamed {
            who: by,
            chat_display_name,
        } => format!("{} renamed the chat to “{chat_display_name}”", who(by)),
        D::CallStarted { meeting: true } => "Meeting started".into(),
        D::CallStarted { meeting: false } => "Call started".into(),
        D::CallEnded { .. } if call_missed_detail(detail, me) => "Missed call".into(),
        D::CallEnded {
            meeting, duration, ..
        } => {
            let ended = if *meeting {
                "Meeting ended"
            } else {
                "Call ended"
            };
            match duration.as_deref().and_then(call_duration) {
                Some(duration) => format!("{ended} after {duration}"),
                None => ended.into(),
            }
        }
        D::CallRecorded { .. } => "Call recorded".into(),
        D::TranscriptAvailable => "Transcript available".into(),
        D::MeetingPolicyUpdated => "Meeting settings changed".into(),
        D::MessagePinned { who: by } => format!("{} pinned a message", who(by)),
        D::MessageUnpinned { who: by } => format!("{} unpinned a message", who(by)),
        D::ConversationRenamed { who: by } => format!("{} renamed the conversation", who(by)),
        D::ChannelAdded { who: by } => format!("{} added a channel", who(by)),
        D::TabUpdated { who: by } => format!("{} updated a tab", who(by)),
        D::Members { .. } | D::Other => "Chat event".into(),
    }
}

/// People added to or removed from a chat, by id and, when Graph says, by
/// name. Graph often leaves `displayName` empty in these events.
fn membership(detail: &g::EventDetail) -> Option<crate::model::Membership> {
    use crate::model::{EventPerson, MemberChange, Membership};
    let g::EventDetail::Members { kind, by, members } = detail else {
        return None;
    };
    let person = |person: &g::EventPerson| EventPerson {
        id: person.id.clone(),
        name: person.name.clone(),
    };
    Some(Membership {
        change: match kind {
            g::MembershipKind::Added => MemberChange::Added,
            g::MembershipKind::Removed => MemberChange::Removed,
            g::MembershipKind::Joined => MemberChange::Joined,
            g::MembershipKind::Left => MemberChange::Left,
        },
        by: by.as_ref().map(person),
        members: members.iter().map(person).collect(),
    })
}

/// A call or meeting starting or ending, for its icon and duration.
fn call_event(detail: &g::EventDetail) -> Option<crate::model::CallEvent> {
    match detail {
        g::EventDetail::CallStarted { meeting } => Some(crate::model::CallEvent {
            meeting: *meeting,
            ended: false,
            duration: None,
        }),
        g::EventDetail::CallEnded {
            meeting, duration, ..
        } => Some(crate::model::CallEvent {
            meeting: *meeting,
            ended: true,
            duration: duration.as_deref().and_then(call_duration),
        }),
        _ => None,
    }
}

/// Whether a recording link may be offered: https, no userinfo, and a
/// SharePoint host (where Teams stores recordings). Anything else is dropped.
pub fn recording_url_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
    if !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let host = host.to_ascii_lowercase();
    // Browsers read a backslash and other odd characters in ways a suffix
    // check would not, so only plain host characters and no empty label pass.
    let plain = host
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    plain
        && host.split('.').all(|label| !label.is_empty())
        && (host == "sharepoint.com" || host.ends_with(".sharepoint.com"))
}

/// A finished meeting recording's link, when Graph gave an allowed one.
fn recording(detail: &g::EventDetail) -> Option<crate::model::Recording> {
    let g::EventDetail::CallRecorded {
        url: Some(url),
        name,
        duration,
        success: true,
    } = detail
    else {
        return None;
    };
    if !recording_url_allowed(url) {
        return None;
    }
    Some(crate::model::Recording {
        url: url.clone(),
        name: name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("Meeting recording")
            .to_owned(),
        duration: duration.as_deref().and_then(call_duration),
    })
}

/// Whether an ended call went unanswered by anyone: a call (not a meeting)
/// that someone else placed and nobody joined besides them. Graph has no
/// answered or declined state, so this is inferred: a
/// decline looks the same, and a group call one other person answered does
/// not count as missed.
pub fn call_missed(detail: Option<&serde_json::Value>, me: &str) -> bool {
    call_missed_detail(&detail_of(detail), me)
}

fn call_missed_detail(detail: &g::EventDetail, me: &str) -> bool {
    let g::EventDetail::CallEnded {
        meeting,
        participants,
        initiator_id,
        ..
    } = detail
    else {
        return false;
    };
    !me.is_empty() && !meeting && *participants < 2 && initiator_id.as_deref() != Some(me)
}

/// How long a call lasted ("4m 12s"), from its ISO 8601 `callDuration`.
fn call_duration(iso: &str) -> Option<String> {
    let seconds = iso_duration_seconds(iso)?;
    (seconds > 0).then(|| short_duration(seconds))
}

/// Seconds in an ISO 8601 duration such as `PT1H4M12.5S` or `P1DT2H`.
fn iso_duration_seconds(text: &str) -> Option<u64> {
    let rest = text.strip_prefix('P')?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    if date.is_empty() && time.is_empty() {
        return None;
    }
    let sum = |part: &str, units: &[(char, u64)]| -> Option<u64> {
        let mut total = 0;
        let mut number = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                number.push(c);
                continue;
            }
            let (_, factor) = units.iter().find(|(unit, _)| *unit == c)?;
            let value: f64 = number.parse().ok()?;
            total += (value * *factor as f64) as u64;
            number.clear();
        }
        number.is_empty().then_some(total)
    };
    Some(sum(date, &[('D', 86_400)])? + sum(time, &[('H', 3600), ('M', 60), ('S', 1)])?)
}

/// "45s", "4m 12s", "1h 5m", "2h".
fn short_duration(seconds: u64) -> String {
    let (hours, minutes, rest) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    match (hours, minutes, rest) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, 0, _) => format!("{h}h"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

pub fn message(
    mut message: g::ChatMessage,
    conversation: Option<&ConversationId>,
    me: &str,
    base: &str,
) -> Message {
    let system = message
        .message_type
        .as_deref()
        .is_some_and(|kind| kind != "message");
    let deleted = message.deleted_date_time.is_some();
    let mut body = if system {
        html::RichText::plain(&system_summary(
            &detail_of(message.event_detail.as_ref()),
            me,
        ))
    } else if deleted {
        html::RichText::default()
    } else {
        body_text(message.body.as_ref())
    };
    let mut reactions: Vec<Reaction> = Vec::new();
    for reaction in &message.reactions {
        let emoji = reaction_emoji(&reaction.reaction_type);
        let user = reaction
            .user
            .as_ref()
            .and_then(|u| u.id())
            .unwrap_or_default()
            .to_owned();
        match reactions.iter_mut().find(|r| r.emoji == emoji) {
            Some(existing) => existing.users.push(user),
            None => reactions.push(Reaction {
                emoji,
                users: vec![user],
            }),
        }
    }
    let attachments = message.attachments.iter().filter_map(attachment).collect();
    let conversation = conversation.cloned().or_else(|| {
        if let Some(chat) = &message.chat_id {
            return Some(ConversationId::Chat(chat.clone()));
        }
        let identity = message.channel_identity.as_ref()?;
        Some(ConversationId::Channel {
            team: identity.team_id.clone()?,
            channel: identity.channel_id.clone()?,
        })
    });
    if let Some(conversation) = &conversation {
        for block in &mut body.blocks {
            if let html::Block::Image { src, .. } = block
                && let Some(content) = src
                    .strip_prefix("../hostedContents/")
                    .and_then(|s| s.strip_suffix("/$value"))
            {
                let path = match conversation {
                    ConversationId::Chat(chat) => format!(
                        "chats/{}/messages/{}",
                        super::segment(chat),
                        super::segment(&message.id)
                    ),
                    ConversationId::Channel { team, channel } => {
                        let base = format!(
                            "teams/{}/channels/{}/messages",
                            super::segment(team),
                            super::segment(channel)
                        );
                        match &message.reply_to_id {
                            Some(root) => format!(
                                "{}/{}/replies/{}",
                                base,
                                super::segment(root),
                                super::segment(&message.id)
                            ),
                            None => format!("{}/{}", base, super::segment(&message.id)),
                        }
                    }
                };
                *src = format!(
                    "{}/{}/hostedContents/{}/$value",
                    base,
                    path,
                    super::segment(content)
                );
            }
        }
    }
    let created = message
        .created_date_time
        .as_deref()
        .and_then(parse_iso)
        .unwrap_or_default();
    let replies = message
        .replies
        .unwrap_or_default()
        .into_iter()
        .map(|reply| self::message(reply, conversation.as_ref(), me, base))
        .collect::<Vec<_>>();
    let mut replies = replies;
    replies.sort_by_key(|reply| reply.created);
    let event = detail_of(message.event_detail.as_ref());
    let pin_event = system
        && matches!(
            event,
            g::EventDetail::MessagePinned { .. } | g::EventDetail::MessageUnpinned { .. }
        );
    let missed_call = system && call_missed_detail(&event, me);
    let mentions = std::mem::take(&mut message.mentions);
    Message {
        web_url: message.web_url.filter(|url| !url.is_empty()),
        pin_event,
        missed_call,
        membership: system.then(|| membership(&event)).flatten(),
        call: system.then(|| call_event(&event)).flatten(),
        recording: system.then(|| recording(&event)).flatten(),
        author_id: message
            .from
            .as_ref()
            .and_then(|f| f.id())
            .map(str::to_owned),
        author: message
            .from
            .as_ref()
            .and_then(|f| f.name())
            .unwrap_or(if system { "" } else { "Unknown" })
            .to_owned(),
        created,
        modified: message
            .last_modified_date_time
            .as_deref()
            .and_then(parse_iso)
            .unwrap_or(created),
        edited: message.last_edited_date_time.is_some(),
        deleted,
        system,
        subject: message.subject.filter(|s| !s.trim().is_empty()),
        importance_high: matches!(message.importance.as_deref(), Some("high" | "urgent")),
        urgent: message.importance.as_deref() == Some("urgent"),
        mentions: mentions
            .iter()
            .filter_map(|m| m.mentioned.as_ref()?.user.as_ref()?.id.clone())
            .collect(),
        mentions_everyone: mentions.iter().any(|m| {
            m.mentioned
                .as_ref()
                .is_some_and(|m| m.conversation.is_some())
        }),
        body,
        reactions,
        attachments,
        replies,
        id: message.id,
        reply_to: message.reply_to_id,
        conversation,
        delivery: Default::default(),
    }
}

fn attachment(attachment: &g::Attachment) -> Option<Attachment> {
    let content_type = attachment.content_type.clone();
    let kind = match content_type.as_deref() {
        Some("messageReference") => {
            let content: serde_json::Value = attachment
                .content
                .as_deref()
                .and_then(|c| serde_json::from_str(c).ok())
                .unwrap_or_default();
            AttachmentKind::Quote {
                message_id: content
                    .get("messageId")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .or_else(|| attachment.id.clone())
                    .unwrap_or_default(),
                author: content
                    .pointer("/messageSender/user/displayName")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                preview: content
                    .get("messagePreview")
                    .and_then(|v| v.as_str())
                    .map(|text| html::parse(text).summary(200))
                    .unwrap_or_default(),
            }
        }
        Some("reference") => AttachmentKind::File,
        Some(kind) if kind.starts_with("application/vnd.microsoft.card") => AttachmentKind::Card,
        Some("application/vnd.microsoft.teams.messaging-announcementBanner") => return None,
        Some(kind) if kind.contains('/') => AttachmentKind::File,
        _ => AttachmentKind::Other,
    };
    let kind_is_card = matches!(kind, AttachmentKind::Card);
    Some(Attachment {
        id: attachment.id.clone().unwrap_or_default(),
        kind,
        name: attachment.name.clone(),
        content_type,
        url: attachment.content_url.clone(),
        content: kind_is_card.then(|| attachment.content.clone()).flatten(),
    })
}

/// A pin with its message, for the banner: the first 200 characters of the
/// text. A pin whose message Graph left out is dropped.
pub fn pinned_message(pin: g::PinnedChatMessage) -> Option<PinnedMessage> {
    let message = pin.message?;
    Some(PinnedMessage {
        id: pin.id,
        message_id: message.id,
        author: message
            .from
            .as_ref()
            .and_then(|f| f.name())
            .unwrap_or_default()
            .to_owned(),
        preview: body_text(message.body.as_ref()).summary(200),
        at: message
            .created_date_time
            .as_deref()
            .and_then(parse_iso)
            .unwrap_or_default(),
    })
}

pub fn team(team: g::Team) -> Team {
    Team {
        name: team.display_name.unwrap_or_else(|| "Untitled team".into()),
        description: team.description.filter(|d| !d.trim().is_empty()),
        web_url: team.web_url,
        id: team.id,
    }
}

pub fn channel(channel: g::Channel, team_id: &str) -> Channel {
    Channel {
        name: channel
            .display_name
            .unwrap_or_else(|| "Untitled channel".into()),
        description: channel.description.filter(|d| !d.trim().is_empty()),
        kind: match channel.membership_type.as_deref() {
            Some("private") => ChannelKind::Private,
            Some("shared") => ChannelKind::Shared,
            _ => ChannelKind::Standard,
        },
        web_url: channel.web_url,
        team_id: team_id.to_owned(),
        id: channel.id,
    }
}

pub fn presence(presence: g::Presence) -> (String, Presence) {
    (
        presence.id,
        Presence {
            availability: Availability::from_graph(
                presence.availability.as_deref().unwrap_or_default(),
            ),
            activity: presence.activity,
            status_message: presence
                .status_message
                .and_then(|s| s.message)
                .and_then(|m| m.content)
                .map(|c| html::parse(&c).summary(120))
                .filter(|c| !c.is_empty()),
        },
    )
}

pub fn meeting(event: g::Event) -> Option<Meeting> {
    let start = event.start.as_ref()?;
    let end = event.end.as_ref()?;
    let join_url = event
        .online_meeting
        .and_then(|m| m.join_url)
        .or(event.online_meeting_url)
        .filter(|url| crate::calling::is_teams_link(url));
    Some(Meeting {
        subject: event
            .subject
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "(No title)".into()),
        start: parse_civil(&start.date_time, start.time_zone.as_deref())?,
        end: parse_civil(&end.date_time, end.time_zone.as_deref())?,
        all_day: event.is_all_day.unwrap_or(false),
        location: event
            .location
            .and_then(|l| l.display_name)
            .filter(|l| !l.trim().is_empty()),
        organizer: event
            .organizer
            .and_then(|o| o.email_address)
            .and_then(|e| e.name.or(e.address)),
        join_url,
        web_link: event.web_link,
        cancelled: event.is_cancelled.unwrap_or(false),
        preview: event.body_preview.filter(|p| !p.trim().is_empty()),
        id: event.id,
    })
}

/// Automatic replies, with their HTML messages as plain text.
pub fn out_of_office(raw: g::AutomaticRepliesSetting) -> OutOfOffice {
    let when = |at: Option<&g::DateTimeTimeZone>| {
        at.filter(|at| !at.date_time.is_empty())
            .and_then(|at| parse_civil(&at.date_time, at.time_zone.as_deref()))
    };
    let text = |message: Option<String>| {
        html::parse(message.as_deref().unwrap_or_default())
            .to_plain()
            .trim()
            .to_owned()
    };
    OutOfOffice {
        replies: match raw.status.as_deref() {
            Some("alwaysEnabled") => AutoReplies::On,
            Some("scheduled") => AutoReplies::Scheduled,
            _ => AutoReplies::Off,
        },
        start: when(raw.scheduled_start_date_time.as_ref()),
        end: when(raw.scheduled_end_date_time.as_ref()),
        internal: text(raw.internal_reply_message),
        external: text(raw.external_reply_message),
        audience: match raw.external_audience.as_deref() {
            Some("all") => ExternalAudience::All,
            Some("contactsOnly") => ExternalAudience::Contacts,
            _ => ExternalAudience::None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_bodies_drop_attachment_placeholders() {
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "m", "body": {"contentType": "text", "content":
                "What do you think?<attachment id=\"114bdbee-dd0c\"></attachment>"}
        }))
        .unwrap();
        let message = message(
            raw,
            Some(&ConversationId::Chat("chat".into())),
            "u-me",
            crate::graph::GRAPH_BASE,
        );
        assert_eq!(message.body.summary(200), "What do you think?");
        assert_eq!(
            without_attachment_tags("a <attachment id=\"x\">"),
            "a <attachment id=\"x\">"
        );
        assert_eq!(
            without_attachment_tags(
                "<attachment id=\"1\"></attachment>a<attachment id=\"2\"></attachment> b"
            ),
            "a b"
        );
    }

    #[test]
    fn relative_hosted_images_use_the_message_path_and_external_images_stay_external() {
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id":"message", "body":{"contentType":"html", "content":"<img src=\"../hostedContents/1/$value\"><img src=\"https://outside.example/image.png\">"}
        })).unwrap();
        let message = message(
            raw,
            Some(&ConversationId::Chat("chat".into())),
            "u-me",
            crate::graph::GRAPH_BASE,
        );
        assert_eq!(
            message.body.images().collect::<Vec<_>>(),
            [
                "https://graph.microsoft.com/v1.0/chats/chat/messages/message/hostedContents/1/$value",
                "https://outside.example/image.png",
            ]
        );
    }

    #[test]
    fn chat_with_preview_members_and_viewpoint() {
        let raw: g::Chat = serde_json::from_value(serde_json::json!({
            "id": "19:abc@thread.v2",
            "topic": null,
            "chatType": "oneOnOne",
            "lastUpdatedDateTime": "2026-10-10T09:00:00Z",
            "webUrl": "https://teams.microsoft.com/l/chat/19%3Aabc%40thread.v2/0",
            "members": [
                {"@odata.type": "#microsoft.graph.aadUserConversationMember", "id": "m1", "displayName": "Ada", "userId": "u-ada", "email": "ada@contoso.com", "tenantId": "t-ada"},
                {"@odata.type": "#microsoft.graph.aadUserConversationMember", "id": "m2", "displayName": "Me", "userId": "u-me", "email": "me@contoso.com"}
            ],
            "lastMessagePreview": {
                "id": "1700", "createdDateTime": "2026-10-10T10:00:00Z", "isDeleted": false, "messageType": "message",
                "body": {"contentType": "html", "content": "<p>Ship <b>it</b></p>"},
                "from": {"user": {"id": "u-ada", "displayName": "Ada"}}
            },
            "viewpoint": {"isHidden": false, "lastMessageReadDateTime": "2026-10-10T09:30:00Z"}
        }))
        .unwrap();
        let chat = chat(raw, "u-me");
        assert_eq!(chat.title("u-me"), "Ada");
        assert_eq!(chat.members[0].tenant_id.as_deref(), Some("t-ada"));
        assert_eq!(chat.members[1].tenant_id, None);
        assert_eq!(chat.preview.as_ref().unwrap().text, "Ship it");
        assert!(chat.is_unread("u-me"));
        assert_eq!(
            chat.peer("u-me").unwrap().email.as_deref(),
            Some("ada@contoso.com")
        );
    }

    #[test]
    fn message_with_reactions_quote_and_edit() {
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "1701", "replyToId": null, "messageType": "message",
            "createdDateTime": "2026-10-10T10:00:00Z", "lastModifiedDateTime": "2026-10-10T10:05:00Z",
            "lastEditedDateTime": "2026-10-10T10:04:00Z", "deletedDateTime": null,
            "chatId": "19:abc@thread.v2", "importance": "high",
            "from": {"user": {"id": "u-ada", "displayName": "Ada", "userIdentityType": "aadUser"}},
            "body": {"contentType": "html", "content": "<attachment id=\"1690\"></attachment><p>Agreed</p>"},
            "attachments": [{"id": "1690", "contentType": "messageReference", "content": "{\"messageId\":\"1690\",\"messagePreview\":\"Can we ship Friday?\",\"messageSender\":{\"user\":{\"id\":\"u-me\",\"displayName\":\"Me\"}}}"}],
            "reactions": [
                {"reactionType": "like", "createdDateTime": "2026-10-10T10:01:00Z", "user": {"user": {"id": "u-me"}}},
                {"reactionType": "👍", "createdDateTime": "2026-10-10T10:02:00Z", "user": {"user": {"id": "u-grace"}}},
                {"reactionType": "🎉", "user": {"user": {"id": "u-me"}}}
            ]
        }))
        .unwrap();
        let message = message(raw, None, "u-me", crate::graph::GRAPH_BASE);
        assert_eq!(
            message.conversation,
            Some(ConversationId::Chat("19:abc@thread.v2".into()))
        );
        assert!(message.edited && message.importance_high);
        assert!(!message.urgent && message.mentions.is_empty() && !message.mentions_everyone);
        assert_eq!(message.body.to_plain(), "Agreed");
        assert_eq!(message.reactions.len(), 2);
        assert_eq!(message.reactions[0].users, vec!["u-me", "u-grace"]);
        let Some(Attachment {
            kind: AttachmentKind::Quote {
                preview, author, ..
            },
            ..
        }) = message.quote()
        else {
            panic!("quote expected");
        };
        assert_eq!(preview, "Can we ship Friday?");
        assert_eq!(author.as_deref(), Some("Me"));
    }

    #[test]
    fn system_events_get_words() {
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "1", "messageType": "systemEventMessage", "createdDateTime": "2026-10-10T10:00:00Z",
            "body": {"contentType": "html", "content": "<systemEventMessage/>"},
            "eventDetail": {
                "@odata.type": "#microsoft.graph.membersAddedEventMessageDetail",
                "members": [{"id": "u-g", "displayName": "Grace"}],
                "initiator": {"user": {"id": "u-ada", "displayName": "Ada"}}
            }
        }))
        .unwrap();
        let message = message(raw, None, "u-me", crate::graph::GRAPH_BASE);
        assert!(message.system);
        assert_eq!(message.body.to_plain(), "Ada added Grace to the chat.");
    }

    #[test]
    fn membership_events_keep_ids_for_names_graph_left_out() {
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "1", "messageType": "systemEventMessage", "createdDateTime": "2026-10-10T10:00:00Z",
            "body": {"contentType": "html", "content": "<systemEventMessage/>"},
            "eventDetail": {
                "@odata.type": "#microsoft.graph.membersDeletedEventMessageDetail",
                "members": [{"id": "u-g", "displayName": null}],
                "initiator": {"user": {"id": "u-ada", "displayName": null}}
            }
        }))
        .unwrap();
        let message = message(raw, None, "u-me", crate::graph::GRAPH_BASE);
        assert_eq!(
            message.body.to_plain(),
            "Someone removed someone from the chat."
        );
        let membership = message.membership.expect("kept for naming");
        assert_eq!(membership.unnamed().collect::<Vec<_>>(), ["u-ada", "u-g"]);
        let names = |id: &str| match id {
            "u-ada" => Some("Ada".to_owned()),
            "u-g" => Some("Grace".to_owned()),
            _ => None,
        };
        assert_eq!(
            membership.describe(names),
            "Ada removed Grace from the chat."
        );
    }

    #[test]
    fn call_events_keep_their_kind_and_length() {
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "1", "messageType": "systemEventMessage", "createdDateTime": "2026-10-10T10:00:00Z",
            "body": {"contentType": "html", "content": "<systemEventMessage/>"},
            "eventDetail": {
                "@odata.type": "#microsoft.graph.callEndedEventMessageDetail",
                "callEventType": "meeting",
                "callDuration": "PT8M18S",
                "callParticipants": [{"participant": {"user": {"id": "u-ada"}}}],
                "initiator": {"user": {"id": "u-ada"}}
            }
        }))
        .unwrap();
        let call = message(raw, None, "u-me", crate::graph::GRAPH_BASE)
            .call
            .expect("a call event");
        assert!(call.meeting && call.ended);
        assert_eq!(call.duration.as_deref(), Some("8m 18s"));
    }

    fn call_ended(detail: serde_json::Value) -> Message {
        let mut detail = detail;
        detail["@odata.type"] = "#microsoft.graph.callEndedEventMessageDetail".into();
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "1", "messageType": "systemEventMessage", "createdDateTime": "2026-10-10T10:00:00Z",
            "body": {"contentType": "html", "content": "<systemEventMessage/>"},
            "eventDetail": detail
        }))
        .unwrap();
        message(raw, None, "u-me", crate::graph::GRAPH_BASE)
    }

    fn recorded(detail: serde_json::Value) -> Message {
        let mut detail = detail;
        detail["@odata.type"] = "#microsoft.graph.callRecordingEventMessageDetail".into();
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "1", "messageType": "systemEventMessage", "createdDateTime": "2026-10-10T10:00:00Z",
            "body": {"contentType": "html", "content": "<systemEventMessage/>"},
            "eventDetail": detail
        }))
        .unwrap();
        message(raw, None, "u-me", crate::graph::GRAPH_BASE)
    }

    #[test]
    fn recorded_event_becomes_a_recording() {
        let m = recorded(serde_json::json!({
            "callRecordingUrl": "https://c-my.sharepoint.com/r.mp4",
            "callRecordingDisplayName": "Sync",
            "callRecordingDuration": "PT58M",
            "callRecordingStatus": "success"
        }));
        assert_eq!(m.body.to_plain(), "Call recorded");
        assert_eq!(
            m.recording,
            Some(crate::model::Recording {
                url: "https://c-my.sharepoint.com/r.mp4".into(),
                name: "Sync".into(),
                duration: Some("58m".into()),
            })
        );
        for name in [serde_json::json!(""), serde_json::json!(null)] {
            let m = recorded(serde_json::json!({
                "callRecordingUrl": "https://c-my.sharepoint.com/r.mp4",
                "callRecordingDisplayName": name,
                "callRecordingStatus": "success"
            }));
            assert_eq!(m.recording.unwrap().name, "Meeting recording");
        }
        let m = recorded(serde_json::json!({
            "callRecordingUrl": "https://c-my.sharepoint.com/r.mp4",
            "callRecordingStatus": "success"
        }));
        assert_eq!(m.recording.unwrap().name, "Meeting recording");
    }

    #[test]
    fn recording_needs_success_status() {
        for status in ["initial", "chunkFinished", "failure"] {
            let m = recorded(serde_json::json!({
                "callRecordingUrl": "https://c-my.sharepoint.com/r.mp4",
                "callRecordingStatus": status
            }));
            assert_eq!(m.recording, None, "{status}");
            assert_eq!(m.body.to_plain(), "Call recorded");
        }
        let m = recorded(serde_json::json!({
            "callRecordingUrl": "https://c-my.sharepoint.com/r.mp4"
        }));
        assert_eq!(m.recording, None);
    }

    #[test]
    fn recording_url_rejects_lookalikes() {
        for url in [
            "https://sharepoint.com.evil.com/a",
            "https://user@evil.com/.sharepoint.com",
            "https://evil.com@contoso.sharepoint.com/a",
            "https://evil.com/x?h=.sharepoint.com",
            "https://evilsharepoint.com/a",
            "https://evil.com\\.sharepoint.com/a",
            "https://evil.com%2f.sharepoint.com/a",
            "https://evil.com .sharepoint.com/a",
            "https://a..sharepoint.com/x",
            "https://contoso.sharepoint.com./a",
            "https://\u{212A}.sharepoint.com/a",
            "https://xn--80ak6aa92e.sharepoint.com\t.evil.com/a",
            "https://[::1]/a",
            "HTTPS://contoso.sharepoint.com/a",
            "https://a.sharepoint.com:44x/x",
            "http://contoso.sharepoint.com/a",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!recording_url_allowed(url), "{url}");
        }
        assert!(recording_url_allowed(
            "https://CONTOSO.SharePoint.com:443/a"
        ));
        assert!(recording_url_allowed(
            "https://contoso-my.sharepoint.com/personal/a/b.mp4"
        ));
        assert!(recording_url_allowed("https://sharepoint.com/a"));
    }

    #[test]
    fn unknown_event_detail_keeps_message() {
        for detail in [
            serde_json::json!({"@odata.type": "#microsoft.graph.somethingNew"}),
            serde_json::json!({"@odata.type": 7, "members": "x"}),
            serde_json::json!("not an object"),
        ] {
            let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
                "id": "1", "messageType": "systemEventMessage", "createdDateTime": "2026-10-10T10:00:00Z",
                "body": {"contentType": "html", "content": "<systemEventMessage/>"},
                "eventDetail": detail
            }))
            .unwrap();
            let converted = message(raw, None, "u-me", crate::graph::GRAPH_BASE);
            assert!(converted.system);
            assert_eq!(converted.body.to_plain(), "Chat event");
            assert!(converted.membership.is_none() && converted.call.is_none());
        }
    }

    #[test]
    fn an_unanswered_call_from_someone_else_is_missed() {
        let missed = call_ended(serde_json::json!({
            "callEventType": "call", "callDuration": "PT0S",
            "initiator": {"user": {"id": "u-ada"}},
            "callParticipants": [{"participant": {"user": {"id": "u-ada"}}}]
        }));
        assert!(missed.missed_call);
        assert_eq!(missed.body.to_plain(), "Missed call");

        // Nobody joined, but nobody is named as the caller either.
        let unknown = call_ended(serde_json::json!({"callEventType": "call"}));
        assert!(unknown.missed_call);
    }

    #[test]
    fn calls_i_placed_answered_calls_and_meetings_are_not_missed() {
        let mine = call_ended(serde_json::json!({
            "callEventType": "call", "initiator": {"user": {"id": "u-me"}},
            "callParticipants": [{"participant": {"user": {"id": "u-me"}}}]
        }));
        assert!(!mine.missed_call);
        assert_eq!(mine.body.to_plain(), "Call ended");

        let answered = call_ended(serde_json::json!({
            "callEventType": "call", "callDuration": "PT4M12.5S",
            "initiator": {"user": {"id": "u-ada"}},
            "callParticipants": [
                {"participant": {"user": {"id": "u-ada"}}},
                {"participant": {"user": {"id": "u-me"}}}
            ]
        }));
        assert!(!answered.missed_call);
        assert_eq!(answered.body.to_plain(), "Call ended after 4m 12s");

        let meeting = call_ended(serde_json::json!({
            "callEventType": "meeting", "callDuration": "PT1H5M30S",
            "initiator": {"user": {"id": "u-ada"}}
        }));
        assert!(!meeting.missed_call);
        assert_eq!(meeting.body.to_plain(), "Meeting ended after 1h 5m");

        // Without the signed-in id nothing is called missed.
        assert!(!call_missed(
            Some(&serde_json::json!({
                "@odata.type": "#microsoft.graph.callEndedEventMessageDetail",
                "callEventType": "call"
            })),
            ""
        ));
    }

    #[test]
    fn missed_calls_show_in_the_chat_list_preview() {
        let raw: g::Chat = serde_json::from_value(serde_json::json!({
            "id": "19:a_b@unq.gbl.spaces", "chatType": "oneOnOne",
            "lastMessagePreview": {
                "id": "1", "createdDateTime": "2026-10-10T10:00:00Z", "isDeleted": false,
                "messageType": "systemEventMessage",
                "eventDetail": {
                    "@odata.type": "#microsoft.graph.callEndedEventMessageDetail",
                    "callEventType": "call", "initiator": {"user": {"id": "u-ada"}},
                    "callParticipants": []
                }
            }
        }))
        .unwrap();
        assert_eq!(chat(raw, "u-me").preview.unwrap().text, "Missed call");
    }

    #[test]
    fn durations_read_iso_8601_and_print_short() {
        assert_eq!(iso_duration_seconds("PT45S"), Some(45));
        assert_eq!(iso_duration_seconds("PT4M12.9S"), Some(252));
        assert_eq!(iso_duration_seconds("P1DT2H"), Some(93_600));
        assert_eq!(iso_duration_seconds("PT"), None);
        assert_eq!(iso_duration_seconds("P"), None);
        assert_eq!(iso_duration_seconds("4M"), None);
        assert_eq!(iso_duration_seconds("PT4X"), None);
        assert_eq!(short_duration(45), "45s");
        assert_eq!(short_duration(240), "4m");
        assert_eq!(short_duration(252), "4m 12s");
        assert_eq!(short_duration(7200), "2h");
        assert_eq!(short_duration(3930), "1h 5m");
        // A zero-length call shows no duration.
        assert_eq!(
            call_ended(serde_json::json!({
                "callEventType": "call", "callDuration": "PT0S",
                "initiator": {"user": {"id": "u-me"}}
            }))
            .body
            .to_plain(),
            "Call ended"
        );
    }

    #[test]
    fn deleted_messages_have_no_body() {
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "1", "messageType": "message", "deletedDateTime": "2026-10-10T10:00:00Z",
            "body": {"contentType": "html", "content": "<p>secret</p>"}
        }))
        .unwrap();
        let message = message(raw, None, "u-me", crate::graph::GRAPH_BASE);
        assert!(message.deleted);
        assert!(message.body.is_empty());
    }

    #[test]
    fn meetings_keep_only_teams_join_links() {
        let raw: g::Event = serde_json::from_value(serde_json::json!({
            "id": "e1", "subject": "Standup",
            "start": {"dateTime": "2026-10-12T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-10-12T09:15:00.0000000", "timeZone": "UTC"},
            "isOnlineMeeting": true,
            "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0"}
        }))
        .unwrap();
        let meeting = meeting(raw).unwrap();
        assert!(meeting.join_url.is_some());
        assert_eq!(meeting.end - meeting.start, 15 * 60_000);

        let phishy: g::Event = serde_json::from_value(serde_json::json!({
            "id": "e2", "start": {"dateTime": "2026-10-12T09:00:00", "timeZone": "UTC"},
            "end": {"dateTime": "2026-10-12T10:00:00", "timeZone": "UTC"},
            "onlineMeeting": {"joinUrl": "https://evil.example/l/meetup-join/x"}
        }))
        .unwrap();
        assert_eq!(meeting_of(phishy).join_url, None);
    }

    fn meeting_of(event: g::Event) -> Meeting {
        meeting(event).unwrap()
    }

    #[test]
    fn chat_message_web_url_and_pin_event_convert() {
        let plain: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "1", "messageType": "message", "chatId": "c",
            "webUrl": "https://teams.microsoft.com/l/message/c/1"
        }))
        .unwrap();
        let plain = message(plain, None, "u-me", crate::graph::GRAPH_BASE);
        assert_eq!(
            plain.web_url.as_deref(),
            Some("https://teams.microsoft.com/l/message/c/1")
        );
        assert!(!plain.pin_event);
        for kind in [
            "messagePinnedEventMessageDetail",
            "messageUnpinnedEventMessageDetail",
        ] {
            let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
                "id": "2", "messageType": "systemEventMessage", "chatId": "c",
                "eventDetail": {"@odata.type": format!("#microsoft.graph.{kind}")}
            }))
            .unwrap();
            let event = message(raw, None, "u-me", crate::graph::GRAPH_BASE);
            assert!(event.pin_event, "{kind}");
            assert!(event.system);
        }
    }

    #[test]
    fn card_content_kept() {
        let raw: g::ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "3", "messageType": "message", "chatId": "c",
            "attachments": [
                {"id": "a", "contentType": "application/vnd.microsoft.card.adaptive", "content": "{\"type\":\"AdaptiveCard\"}"},
                {"id": "b", "contentType": "application/pdf", "content": "ignored"}
            ]
        }))
        .unwrap();
        let message = message(raw, None, "u-me", crate::graph::GRAPH_BASE);
        assert_eq!(
            message.attachments[0].content.as_deref(),
            Some("{\"type\":\"AdaptiveCard\"}")
        );
        assert_eq!(message.attachments[1].content, None);
    }

    fn parsed(extra: serde_json::Value) -> Message {
        let mut json = serde_json::json!({"id": "1", "chatId": "19:a@thread.v2"});
        json.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        message(
            serde_json::from_value(json).unwrap(),
            None,
            "u-me",
            crate::graph::GRAPH_BASE,
        )
    }

    #[test]
    fn mentions_parse_user_and_conversation() {
        let m = parsed(serde_json::json!({"mentions": [
            {"id": 0, "mentionText": "Me", "mentioned": {"user": {"id": "u-me"}}},
            {"id": 1, "mentioned": {"user": {"id": "u-ada"}}}
        ]}));
        assert_eq!(m.mentions, vec!["u-me", "u-ada"]);
        assert!(!m.mentions_everyone && m.mentions("u-me") && !m.mentions("u-x"));
        let all = parsed(serde_json::json!({"mentions": [
            {"id": 0, "mentioned": {"conversation": {"id": "19:a@thread.v2", "conversationIdentityType": "chat"}}}
        ]}));
        assert!(all.mentions_everyone && all.mentions.is_empty() && all.mentions("u-x"));
    }

    #[test]
    fn urgent_split_from_high() {
        let urgent = parsed(serde_json::json!({"importance": "urgent"}));
        assert!(urgent.urgent && urgent.importance_high);
        let high = parsed(serde_json::json!({"importance": "high"}));
        assert!(!high.urgent && high.importance_high);
        let normal = parsed(serde_json::json!({"importance": "normal"}));
        assert!(!normal.urgent && !normal.importance_high);
    }

    #[test]
    fn old_cached_message_loads() {
        let mut value = serde_json::to_value(parsed(serde_json::json!({}))).unwrap();
        let object = value.as_object_mut().unwrap();
        for key in ["urgent", "mentions", "mentions_everyone"] {
            assert!(object.remove(key).is_some());
        }
        let old: Message = serde_json::from_value(value).unwrap();
        assert!(!old.urgent && old.mentions.is_empty() && !old.mentions_everyone);
    }
}
