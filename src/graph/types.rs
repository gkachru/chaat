//! Graph's JSON shapes, only as far as Chaat reads them. Every field is
//! optional or defaulted: Graph adds fields freely and leaves many out.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Page<T> {
    #[serde(default = "Vec::new")]
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink", default)]
    pub next_link: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct User {
    pub id: String,
    pub display_name: Option<String>,
    pub mail: Option<String>,
    pub user_principal_name: Option<String>,
    pub job_title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Identity {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdentitySet {
    pub user: Option<Identity>,
    pub application: Option<Identity>,
    pub device: Option<Identity>,
}

impl IdentitySet {
    pub fn id(&self) -> Option<&str> {
        self.user.as_ref().and_then(|u| u.id.as_deref())
    }

    pub fn name(&self) -> Option<&str> {
        [&self.user, &self.application, &self.device]
            .into_iter()
            .flatten()
            .find_map(|identity| identity.display_name.as_deref())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemBody {
    pub content_type: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConversationMember {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMessageInfo {
    pub id: String,
    pub created_date_time: Option<String>,
    pub is_deleted: Option<bool>,
    pub message_type: Option<String>,
    pub event_detail: Option<serde_json::Value>,
    pub body: Option<ItemBody>,
    pub from: Option<IdentitySet>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatViewpoint {
    pub is_hidden: Option<bool>,
    pub last_message_read_date_time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OnlineMeetingInfo {
    pub join_web_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Chat {
    pub id: String,
    pub topic: Option<String>,
    pub chat_type: Option<String>,
    pub created_date_time: Option<String>,
    pub last_updated_date_time: Option<String>,
    pub web_url: Option<String>,
    pub members: Option<Vec<ConversationMember>>,
    pub last_message_preview: Option<ChatMessageInfo>,
    pub viewpoint: Option<ChatViewpoint>,
    pub online_meeting_info: Option<OnlineMeetingInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Attachment {
    pub id: Option<String>,
    pub content_type: Option<String>,
    pub content_url: Option<String>,
    pub content: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reaction {
    pub reaction_type: String,
    pub display_name: Option<String>,
    pub created_date_time: Option<String>,
    pub user: Option<IdentitySet>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelIdentity {
    pub team_id: Option<String>,
    pub channel_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMessage {
    pub id: String,
    pub reply_to_id: Option<String>,
    pub message_type: Option<String>,
    pub created_date_time: Option<String>,
    pub last_modified_date_time: Option<String>,
    pub last_edited_date_time: Option<String>,
    pub deleted_date_time: Option<String>,
    pub subject: Option<String>,
    pub importance: Option<String>,
    pub body: Option<ItemBody>,
    pub from: Option<IdentitySet>,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<Reaction>,
    pub event_detail: Option<serde_json::Value>,
    pub chat_id: Option<String>,
    pub channel_identity: Option<ChannelIdentity>,
    pub replies: Option<Vec<ChatMessage>>,
    pub web_url: Option<String>,
    pub mentions: Vec<ChatMention>,
}

/// One entry of a message's `mentions`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMention {
    pub mentioned: Option<Mentioned>,
}

/// Who or what is mentioned: a user, or a whole conversation (everyone).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mentioned {
    pub user: Option<Identity>,
    pub conversation: Option<serde_json::Value>,
}

/// One entry of `chats/{id}/pinnedMessages?$expand=message`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PinnedChatMessage {
    pub id: String,
    pub message: Option<ChatMessageInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Team {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub web_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Channel {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub membership_type: Option<String>,
    pub web_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusMessageBody {
    pub content: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusMessage {
    pub message: Option<StatusMessageBody>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Presence {
    pub id: String,
    pub availability: Option<String>,
    pub activity: Option<String>,
    pub status_message: Option<StatusMessage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DateTimeTimeZone {
    pub date_time: String,
    pub time_zone: Option<String>,
}

/// `mailboxSettings/automaticRepliesSetting`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutomaticRepliesSetting {
    /// `disabled`, `alwaysEnabled` or `scheduled`.
    pub status: Option<String>,
    /// `none`, `contactsOnly` or `all`.
    pub external_audience: Option<String>,
    pub scheduled_start_date_time: Option<DateTimeTimeZone>,
    pub scheduled_end_date_time: Option<DateTimeTimeZone>,
    /// HTML.
    pub internal_reply_message: Option<String>,
    pub external_reply_message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Location {
    pub display_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EmailAddress {
    pub name: Option<String>,
    pub address: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Recipient {
    pub email_address: Option<EmailAddress>,
}

/// An event's `onlineMeeting`, or an `onlineMeeting` resource found by its
/// join link.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OnlineMeeting {
    pub join_url: Option<String>,
    pub join_web_url: Option<String>,
    pub chat_info: Option<ChatInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfo {
    pub thread_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Event {
    pub id: String,
    pub subject: Option<String>,
    pub body_preview: Option<String>,
    pub start: Option<DateTimeTimeZone>,
    pub end: Option<DateTimeTimeZone>,
    pub is_all_day: Option<bool>,
    pub is_cancelled: Option<bool>,
    pub location: Option<Location>,
    pub organizer: Option<Recipient>,
    pub is_online_meeting: Option<bool>,
    pub online_meeting: Option<OnlineMeeting>,
    pub online_meeting_url: Option<String>,
    pub web_link: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ErrorBody {
    pub error: Option<ErrorDetail>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ErrorDetail {
    pub code: Option<String>,
    pub message: Option<String>,
}

// Microsoft Search.

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchResponse {
    pub value: Vec<SearchResultSet>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchResultSet {
    pub hits_containers: Vec<HitsContainer>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HitsContainer {
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchHit {
    pub hit_id: Option<String>,
    pub summary: Option<String>,
    pub resource: Option<serde_json::Value>,
}

/// A OneDrive file, as Chaat reads it back after an upload.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DriveItem {
    pub id: String,
    pub name: String,
    pub web_url: Option<String>,
    pub web_dav_url: Option<String>,
    pub e_tag: Option<String>,
    pub size: Option<u64>,
}

impl DriveItem {
    /// The attachment id Teams expects: the GUID inside the item's eTag
    /// (`"{GUID},N"`), lowercase.
    pub fn attachment_id(&self) -> Option<String> {
        let tag = self.e_tag.as_deref()?;
        let start = tag.find('{')? + 1;
        let end = start + tag[start..].find('}')?;
        let guid = tag[start..end].to_ascii_lowercase();
        crate::settings::is_guid(&guid).then_some(guid)
    }

    /// Where the message's attachment points.
    pub fn content_url(&self) -> Option<&str> {
        self.web_dav_url.as_deref().or(self.web_url.as_deref())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSession {
    pub upload_url: String,
}

/// A person named in an event detail, by id and (when Graph says) name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPerson {
    pub id: String,
    pub name: Option<String>,
}

/// How a membership event changed the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipKind {
    Added,
    Removed,
    Joined,
    Left,
}

/// A system message's `eventDetail`, as far as Chaat reads it. Built by
/// [`EventDetail::from_value`], which never fails: anything unknown or
/// malformed is [`EventDetail::Other`]. `who` is the initiator's display
/// name, when Graph gives a non-empty one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventDetail {
    Members {
        kind: MembershipKind,
        by: Option<EventPerson>,
        members: Vec<EventPerson>,
    },
    ChatRenamed {
        who: Option<String>,
        chat_display_name: String,
    },
    CallStarted {
        meeting: bool,
    },
    CallEnded {
        meeting: bool,
        /// How many people joined (`callParticipants`).
        participants: usize,
        initiator_id: Option<String>,
        /// ISO 8601 `callDuration`, as sent.
        duration: Option<String>,
    },
    CallRecorded {
        url: Option<String>,
        name: Option<String>,
        /// ISO 8601 `callRecordingDuration`, as sent.
        duration: Option<String>,
        /// `callRecordingStatus` was `success`.
        success: bool,
    },
    TranscriptAvailable,
    MeetingPolicyUpdated,
    MessagePinned {
        who: Option<String>,
    },
    MessageUnpinned {
        who: Option<String>,
    },
    ConversationRenamed {
        who: Option<String>,
    },
    ChannelAdded {
        who: Option<String>,
    },
    TabUpdated {
        who: Option<String>,
    },
    Other,
}

impl EventDetail {
    /// Reads an `eventDetail` value. Never fails: an unknown or missing
    /// `@odata.type`, or a known one with a field of the wrong shape that
    /// Chaat needs (`members` not a list), is `Other`.
    pub fn from_value(value: &serde_json::Value) -> EventDetail {
        let Some(odata) = value.get("@odata.type").and_then(|t| t.as_str()) else {
            return EventDetail::Other;
        };
        let kind = odata.trim_start_matches("#microsoft.graph.");
        let who = || {
            value
                .pointer("/initiator/user/displayName")
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
        };
        let meeting = || value.get("callEventType").and_then(|t| t.as_str()) == Some("meeting");
        let membership = match odata {
            "#microsoft.graph.membersAddedEventMessageDetail" => Some(MembershipKind::Added),
            "#microsoft.graph.membersDeletedEventMessageDetail" => Some(MembershipKind::Removed),
            "#microsoft.graph.membersJoinedEventMessageDetail" => Some(MembershipKind::Joined),
            "#microsoft.graph.membersLeftEventMessageDetail" => Some(MembershipKind::Left),
            _ => None,
        };
        if let Some(kind) = membership {
            let members = match value.get("members") {
                Some(serde_json::Value::Array(list)) => {
                    list.iter().filter_map(Self::person).collect()
                }
                _ => Vec::new(),
            };
            return EventDetail::Members {
                kind,
                by: value.pointer("/initiator/user").and_then(Self::person),
                members,
            };
        }
        match kind {
            "chatRenamedEventMessageDetail" => EventDetail::ChatRenamed {
                who: who(),
                chat_display_name: value
                    .get("chatDisplayName")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_owned(),
            },
            "callStartedEventMessageDetail" => EventDetail::CallStarted { meeting: meeting() },
            "callEndedEventMessageDetail" => EventDetail::CallEnded {
                meeting: meeting(),
                participants: value
                    .get("callParticipants")
                    .and_then(|p| p.as_array())
                    .map_or(0, Vec::len),
                initiator_id: value
                    .pointer("/initiator/user/id")
                    .and_then(|id| id.as_str())
                    .map(str::to_owned),
                duration: value
                    .get("callDuration")
                    .and_then(|d| d.as_str())
                    .map(str::to_owned),
            },
            "callRecordingEventMessageDetail" => {
                let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
                EventDetail::CallRecorded {
                    url: text("callRecordingUrl"),
                    name: text("callRecordingDisplayName"),
                    duration: text("callRecordingDuration"),
                    success: value.get("callRecordingStatus").and_then(|s| s.as_str())
                        == Some("success"),
                }
            }
            "callTranscriptEventMessageDetail" => EventDetail::TranscriptAvailable,
            "meetingPolicyUpdatedEventMessageDetail" => EventDetail::MeetingPolicyUpdated,
            "messagePinnedEventMessageDetail" => EventDetail::MessagePinned { who: who() },
            "messageUnpinnedEventMessageDetail" => EventDetail::MessageUnpinned { who: who() },
            "teamRenamedEventMessageDetail" | "channelRenamedEventMessageDetail" => {
                EventDetail::ConversationRenamed { who: who() }
            }
            "channelAddedEventMessageDetail" => EventDetail::ChannelAdded { who: who() },
            "tabUpdatedEventMessageDetail" => EventDetail::TabUpdated { who: who() },
            _ => EventDetail::Other,
        }
    }

    fn person(value: &serde_json::Value) -> Option<EventPerson> {
        let id = value.get("id").and_then(|i| i.as_str())?;
        Some(EventPerson {
            id: id.to_owned(),
            name: value
                .get("displayName")
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
                .map(str::to_owned),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_event_detail_is_other() {
        let value = json!({"@odata.type": "#microsoft.graph.somethingNew"});
        assert_eq!(EventDetail::from_value(&value), EventDetail::Other);
    }

    #[test]
    fn malformed_event_detail_is_other() {
        let value = json!({"@odata.type": 7, "members": "x"});
        assert_eq!(EventDetail::from_value(&value), EventDetail::Other);
        // A known type with a non-list `members` keeps an empty list, as before.
        let value = json!({
            "@odata.type": "#microsoft.graph.membersAddedEventMessageDetail",
            "members": "x"
        });
        assert_eq!(
            EventDetail::from_value(&value),
            EventDetail::Members {
                kind: MembershipKind::Added,
                by: None,
                members: Vec::new()
            }
        );
        assert_eq!(EventDetail::from_value(&json!("text")), EventDetail::Other);
        assert_eq!(EventDetail::from_value(&json!(null)), EventDetail::Other);
    }

    #[test]
    fn recording_detail_keeps_its_fields() {
        let value = json!({
            "@odata.type": "#microsoft.graph.callRecordingEventMessageDetail",
            "callRecordingUrl": "https://c-my.sharepoint.com/r.mp4",
            "callRecordingDisplayName": "Sync",
            "callRecordingDuration": "PT58M",
            "callRecordingStatus": "success"
        });
        assert_eq!(
            EventDetail::from_value(&value),
            EventDetail::CallRecorded {
                url: Some("https://c-my.sharepoint.com/r.mp4".into()),
                name: Some("Sync".into()),
                duration: Some("PT58M".into()),
                success: true,
            }
        );
    }

    #[test]
    fn missing_odata_type_is_other() {
        assert_eq!(EventDetail::from_value(&json!({})), EventDetail::Other);
    }

    #[test]
    fn known_detail_lands_in_its_variant() {
        let value = json!({
            "@odata.type": "#microsoft.graph.callEndedEventMessageDetail",
            "callEventType": "meeting",
            "callDuration": "PT4M12S",
            "callParticipants": [{}, {}],
            "initiator": {"user": {"id": "u-1", "displayName": "Ada"}}
        });
        assert_eq!(
            EventDetail::from_value(&value),
            EventDetail::CallEnded {
                meeting: true,
                participants: 2,
                initiator_id: Some("u-1".into()),
                duration: Some("PT4M12S".into()),
            }
        );
        let value = json!({
            "@odata.type": "#microsoft.graph.membersAddedEventMessageDetail",
            "initiator": {"user": {"id": "u-1", "displayName": "Ada"}},
            "members": [{"id": "u-2", "displayName": ""}, {"nope": 1}]
        });
        assert_eq!(
            EventDetail::from_value(&value),
            EventDetail::Members {
                kind: MembershipKind::Added,
                by: Some(EventPerson {
                    id: "u-1".into(),
                    name: Some("Ada".into())
                }),
                members: vec![EventPerson {
                    id: "u-2".into(),
                    name: None
                }],
            }
        );
    }
}
