//! Push updates through a chaat-relay.
//!
//! When a relay is configured, Chaat:
//!
//! 1. opens a WebSocket to `{relay}/ws/{channel}` with a random channel;
//! 2. subscribes to Graph change notifications for all of the signed-in
//!    person's chat messages (`/users/{id}/chats/getAllMessages`, Chat.Read,
//!    delegated) and their own presence (`communications/presences/{id}`,
//!    Presence.Read.All, so the call bar follows calls), delivered to
//!    `{relay}/graph/{channel}`, without resource data, so only IDs travel
//!    through the relay;
//! 3. on each notification whose `clientState` matches, fetches the changed
//!    chat with the usual date-ranged request. This is the pattern
//!    Microsoft's polling rules ask for;
//! 4. renews the subscription before it expires, reauthorizes it when Graph
//!    asks, recreates it after Graph removes it, and deletes it on exit.
//!
//! See docs/realtime.md. Not yet exercised against a live tenant.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long each subscription is asked to last. Renewed well before.
pub const LIFETIME: Duration = Duration::from_secs(55 * 60);
pub const RENEW_EVERY: Duration = Duration::from_secs(40 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushStatus {
    Off,
    /// Setting up, at this step.
    Connecting(PushStep),
    /// Subscribed; changes arrive as they happen.
    Live,
    /// Not working; retrying. The reason is for the person.
    Failed(String),
}

impl PushStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Off => "Off".into(),
            Self::Connecting(step) => step.label().into(),
            Self::Live => "Live".into(),
            Self::Failed(reason) => format!("Not connected: {reason}"),
        }
    }
}

/// Where connecting stands, for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushStep {
    /// Opening the connection to the relay.
    Relay,
    /// Deleting subscriptions an earlier run left behind.
    Cleanup,
    /// Waiting for Graph to create the chat subscription; Graph first checks
    /// the relay answers at the notification address.
    Chats,
}

impl PushStep {
    pub fn label(self) -> &'static str {
        match self {
            Self::Relay => "Connecting to the relay…",
            Self::Cleanup => "Removing the last session's subscriptions…",
            Self::Chats => "Waiting for Microsoft Graph to confirm chat updates…",
        }
    }
}

/// Presence updates, which follow the chat subscription: chat updates are
/// live without them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresenceFollow {
    /// Waiting for Graph to create the presence subscription.
    #[default]
    Connecting,
    /// Following this many people.
    Following(usize),
    /// Nobody to follow, or Graph refused; retried at the next renewal.
    NotFollowed,
}

impl PresenceFollow {
    pub fn label(self) -> String {
        match self {
            Self::Connecting => "Presence: waiting for Microsoft Graph…".into(),
            Self::Following(1) => "Presence: following 1 person".into(),
            Self::Following(people) => format!("Presence: following {people} people"),
            Self::NotFollowed => "Presence: not followed".into(),
        }
    }
}

/// The relay's base URL, checked: `https://host[:port][/path]`, or
/// `http://` on the loopback interface for local testing.
pub fn normalize_relay(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let rest = url.strip_prefix("https://").or_else(|| {
        url.strip_prefix("http://").filter(|rest| {
            rest.starts_with("127.0.0.1")
                || rest.starts_with("localhost")
                || rest.starts_with("[::1]")
        })
    })?;
    let host = rest.split('/').next().unwrap_or_default();
    (!host.is_empty() && !host.contains('@') && !url.contains('?') && !url.contains('#'))
        .then(|| url.to_owned())
}

pub fn websocket_url(relay: &str, channel: &str) -> String {
    let base = if let Some(rest) = relay.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = relay.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        relay.to_owned()
    };
    format!("{base}/ws/{channel}")
}

pub fn notification_url(relay: &str, channel: &str) -> String {
    format!("{relay}/graph/{channel}")
}

pub fn lifecycle_url(relay: &str, channel: &str) -> String {
    format!("{relay}/lifecycle/{channel}")
}

pub use crate::relay_protocol::KEY_HEADER;
use crate::relay_protocol::MIN_KEY_LEN;

/// The file, in each account's folder, that lists the subscriptions this
/// installation created and has not deleted yet.
pub const LEFTOVERS_FILE: &str = "push-subscriptions.json";

/// Subscriptions a run of Chaat made. A run that ends without cleaning up
/// (a crash, a killed process) leaves them in Graph for up to an hour,
/// still sending notifications to a channel nobody listens on; the next run
/// deletes them first. Only this installation's own subscriptions are
/// listed, so another device sharing the relay keeps its own.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Leftovers {
    subscriptions: Vec<String>,
}

/// The subscription ids in `path`; none if the file is missing or unreadable.
pub fn read_leftovers(path: &Path) -> Vec<String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Leftovers>(&bytes).ok())
        .map(|leftovers| leftovers.subscriptions)
        .unwrap_or_default()
}

/// Records `ids` in `path`, or removes the file when there are none.
pub fn write_leftovers(path: &Path, ids: &[String]) -> std::io::Result<()> {
    if ids.is_empty() {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let leftovers = Leftovers {
        subscriptions: ids.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&leftovers).map_err(std::io::Error::other)?;
    crate::paths::write_atomically(path, &bytes)
}

/// The file, in each account's folder, listing people Graph refused to follow
/// in a presence subscription, so the next sessions leave them out from the
/// first request.
pub const REFUSED_FILE: &str = "presence-refused.json";

/// How long a refusal is remembered. Graph refuses departed accounts, which
/// stay refused; anyone else (a licence fixed later) is tried again after
/// this.
pub const REFUSAL_MEMORY_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Refusals {
    /// Lower-case user id → when Graph refused them (Unix milliseconds).
    refused: std::collections::BTreeMap<String, i64>,
}

/// The people Graph refused within [`REFUSAL_MEMORY_MS`] of `now`; none if
/// the file is missing or unreadable.
pub fn read_refused(path: &Path, now: i64) -> std::collections::HashMap<String, i64> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Refusals>(&bytes).ok())
        .map(|file| {
            file.refused
                .into_iter()
                .filter(|(id, at)| is_user_id(id) && now.saturating_sub(*at) < REFUSAL_MEMORY_MS)
                .collect()
        })
        .unwrap_or_default()
}

/// Records `refused`, or removes the file when there are none.
pub fn write_refused(
    path: &Path,
    refused: &std::collections::HashMap<String, i64>,
) -> std::io::Result<()> {
    if refused.is_empty() {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let file = Refusals {
        refused: refused.iter().map(|(id, at)| (id.clone(), *at)).collect(),
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
    crate::paths::write_atomically(path, &bytes)
}

/// A relay access key on its way to the credential store. Its `Debug` output
/// hides the key, so a logged command cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub struct RelayKey(pub String);

impl std::fmt::Debug for RelayKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RelayKey(…)")
    }
}

/// Whether `key` can be a relay access key: what the relay accepts (16 or
/// more visible ASCII characters), and short enough for a header.
pub fn valid_relay_key(key: &str) -> bool {
    (MIN_KEY_LEN..=256).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_graphic())
}

/// The WebSocket request for `channel`, carrying the access key if there is
/// one. The key goes in a header, never the URL, so proxies don't log it.
pub fn websocket_request(
    relay: &str,
    channel: &str,
    key: Option<&str>,
) -> Option<tokio_tungstenite::tungstenite::http::Request<()>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = websocket_url(relay, channel).into_client_request().ok()?;
    if let Some(key) = key {
        request.headers_mut().insert(KEY_HEADER, key.parse().ok()?);
    }
    Some(request)
}

/// What a notification asks Chaat to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// A message in this chat was created, edited or deleted.
    Chat(String),
    /// Someone's presence changed: the person's own, or a contact's in the
    /// bulk presence subscription. `None` when Graph did not say whose.
    Presence(Option<String>),
    /// Graph wants this subscription reauthorized (the token behind it is
    /// about to lapse). `None` when the notice named no subscription.
    Reauthorize(Option<String>),
    /// Notifications were lost; refresh the chat list.
    Missed,
    /// Graph removed this subscription; make a new one. `None` when the
    /// notice named no subscription.
    Removed(Option<String>),
}

/// The most chat notices one notification body yields; the rest are
/// dropped. Graph batches a few at a time; a forged body naming hundreds of
/// chats must not become hundreds of fetches.
pub const MAX_CHAT_NOTICES: usize = 20;

#[derive(Deserialize)]
struct Batch {
    #[serde(default)]
    value: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    #[serde(default)]
    subscription_id: Option<String>,
    #[serde(default)]
    client_state: Option<String>,
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    lifecycle_event: Option<String>,
    #[serde(default)]
    resource_data: Option<ResourceData>,
}

#[derive(Deserialize)]
struct ResourceData {
    #[serde(default)]
    id: Option<String>,
}

/// Reads a notification batch. Items whose `clientState` is not ours are
/// dropped. The relay or a push plugin sees `clientState`, so a matching one
/// proves little: the caller still checks lifecycle notices against the
/// subscriptions it holds and limits what they cost. At most
/// `MAX_CHAT_NOTICES` chat notices are kept.
pub fn parse(body: &str, client_state: &str) -> Vec<Notice> {
    let Ok(batch) = serde_json::from_str::<Batch>(body) else {
        return Vec::new();
    };
    let mut notices = Vec::new();
    let mut chats = 0;
    for item in batch.value {
        if item.client_state.as_deref() != Some(client_state) {
            continue;
        }
        let notice = match item.lifecycle_event.as_deref() {
            Some("reauthorizationRequired") => Some(Notice::Reauthorize(item.subscription_id)),
            Some("missed") => Some(Notice::Missed),
            Some("subscriptionRemoved") => Some(Notice::Removed(item.subscription_id)),
            Some(_) => None,
            None => item.resource.as_deref().and_then(|resource| {
                if is_presence(resource) {
                    let user = item
                        .resource_data
                        .and_then(|data| data.id)
                        .or_else(|| presence_user(resource))
                        .filter(|user| is_user_id(user));
                    Some(Notice::Presence(user))
                } else {
                    chat_of(resource).map(Notice::Chat)
                }
            }),
        };
        if let Some(notice) = notice
            && !notices.contains(&notice)
        {
            if matches!(notice, Notice::Chat(_)) {
                if chats == MAX_CHAT_NOTICES {
                    continue;
                }
                chats += 1;
            }
            notices.push(notice);
        }
    }
    notices
}

fn is_presence(resource: &str) -> bool {
    resource
        .trim_start_matches('/')
        .to_ascii_lowercase()
        .starts_with("communications/presences")
}

/// The user in `communications/presences/{id}` or `…/presences('{id}')`.
fn presence_user(resource: &str) -> Option<String> {
    let rest = resource.trim_start_matches('/');
    let rest = rest.get("communications/presences".len()..)?;
    let id = rest
        .strip_prefix("('")
        .and_then(|quoted| quoted.split("')").next())
        .or_else(|| rest.strip_prefix('/'))?;
    (!id.is_empty()).then(|| id.to_owned())
}

/// Entra user ids are GUIDs. Only letters, digits and hyphens are accepted,
/// so an id can never break out of the quotes in a presence `$filter`.
pub fn is_user_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The most people one presence subscription may follow (Graph's limit).
pub const PRESENCE_LIMIT: usize = 650;

/// Who the presence subscription follows: the person first, then contacts
/// in the order given (most recent chats first), without repeats or ids
/// that are not user ids, up to [`PRESENCE_LIMIT`].
pub fn presence_watch(me: &str, contacts: &[String]) -> Vec<String> {
    let mut watch: Vec<String> = Vec::new();
    for id in std::iter::once(me).chain(contacts.iter().map(String::as_str)) {
        if watch.len() == PRESENCE_LIMIT {
            break;
        }
        if is_user_id(id) && !watch.iter().any(|seen| seen.eq_ignore_ascii_case(id)) {
            watch.push(id.to_owned());
        }
    }
    watch
}

/// The chat id in a notification's resource path, in either of Graph's
/// spellings: `chats('19:…')/messages('…')` or `chats/19:…/messages/…`.
fn chat_of(resource: &str) -> Option<String> {
    let rest = resource.trim_start_matches('/');
    let rest = rest.strip_prefix("chats")?;
    let id = if let Some(quoted) = rest.strip_prefix("('") {
        quoted.split("')").next()?
    } else {
        rest.strip_prefix('/')?.split('/').next()?
    };
    let id = urlencoding::decode(id).ok()?.into_owned();
    (!id.is_empty()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftover_subscriptions_round_trip_and_tolerate_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LEFTOVERS_FILE);
        assert!(read_leftovers(&path).is_empty());
        write_leftovers(&path, &["sub-1".into(), "sub-2".into()]).unwrap();
        assert_eq!(read_leftovers(&path), ["sub-1", "sub-2"]);
        write_leftovers(&path, &[]).unwrap();
        assert!(!path.exists());
        write_leftovers(&path, &[]).unwrap();
        std::fs::write(&path, b"not json").unwrap();
        assert!(read_leftovers(&path).is_empty());
        std::fs::write(&path, br#"{"subscriptions":["a"],"later":1}"#).unwrap();
        assert_eq!(read_leftovers(&path), ["a"]);
    }

    #[test]
    fn refusals_are_remembered_for_a_week() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REFUSED_FILE);
        let now = 1_000_000_000_000;
        assert!(read_refused(&path, now).is_empty());
        let mut refused = std::collections::HashMap::new();
        refused.insert("u-gone".to_owned(), now - 1000);
        refused.insert("u-old".to_owned(), now - REFUSAL_MEMORY_MS - 1);
        write_refused(&path, &refused).unwrap();
        let read = read_refused(&path, now);
        assert_eq!(read.len(), 1);
        assert!(read.contains_key("u-gone"));
        write_refused(&path, &Default::default()).unwrap();
        assert!(!path.exists());
        std::fs::write(&path, b"{").unwrap();
        assert!(read_refused(&path, now).is_empty());
    }

    #[test]
    fn relay_keys_travel_in_a_header() {
        assert!(!valid_relay_key("short"));
        assert!(!valid_relay_key("sixteen chars ok"));
        assert!(valid_relay_key("0123456789abcdef"));
        let request = websocket_request(
            "https://relay.example.com",
            "chan",
            Some("0123456789abcdef"),
        )
        .unwrap();
        assert_eq!(request.uri(), "wss://relay.example.com/ws/chan");
        assert_eq!(request.headers()[KEY_HEADER], "0123456789abcdef");
        let open = websocket_request("https://relay.example.com", "chan", None).unwrap();
        assert_eq!(
            format!("{:?}", RelayKey("0123456789abcdef".into())),
            "RelayKey(…)"
        );
        assert!(open.headers().get(KEY_HEADER).is_none());
    }

    #[test]
    fn relay_urls_must_be_https_or_loopback() {
        assert_eq!(
            normalize_relay("https://relay.example.com/"),
            Some("https://relay.example.com".into())
        );
        assert_eq!(
            normalize_relay("http://127.0.0.1:8787"),
            Some("http://127.0.0.1:8787".into())
        );
        assert_eq!(normalize_relay("http://relay.example.com"), None);
        assert_eq!(normalize_relay("https://user@relay.example.com"), None);
        assert_eq!(normalize_relay("ftp://x"), None);
        assert_eq!(
            websocket_url("https://r.example/base", "c"),
            "wss://r.example/base/ws/c"
        );
        assert_eq!(
            notification_url("https://r.example", "c"),
            "https://r.example/graph/c"
        );
    }

    #[test]
    fn notifications_name_their_chat() {
        let body = r#"{"value":[
            {"subscriptionId":"s1","clientState":"ours","changeType":"created","resource":"chats('19:abc@thread.v2')/messages('1700')","resourceData":{"id":"1700"}},
            {"subscriptionId":"s1","clientState":"ours","changeType":"updated","resource":"chats('19:abc@thread.v2')/messages('1701')"},
            {"subscriptionId":"s1","clientState":"theirs","changeType":"created","resource":"chats('19:evil')/messages('1')"},
            {"subscriptionId":"s1","clientState":"ours","changeType":"created","resource":"/chats/19%3Adef%40unq.gbl.spaces/messages/9"}
        ]}"#;
        assert_eq!(
            parse(body, "ours"),
            vec![
                Notice::Chat("19:abc@thread.v2".into()),
                Notice::Chat("19:def@unq.gbl.spaces".into())
            ]
        );
    }

    #[test]
    fn presence_notifications_ask_for_a_presence_fetch() {
        let body = r#"{"value":[
            {"subscriptionId":"s2","clientState":"ours","changeType":"updated","resource":"communications/presences/u-me","resourceData":{"id":"u-me"}},
            {"subscriptionId":"s2","clientState":"ours","changeType":"updated","resource":"/communications/presences/u-me"},
            {"subscriptionId":"s2","clientState":"theirs","changeType":"updated","resource":"communications/presences/u-me"}
        ]}"#;
        assert_eq!(
            parse(body, "ours"),
            vec![Notice::Presence(Some("u-me".into()))]
        );
    }

    #[test]
    fn presence_notices_name_the_person() {
        let me = "3f1b0c2e-0000-4000-8000-000000000001";
        let ada = "3f1b0c2e-0000-4000-8000-0000000000ad";
        let body = format!(
            r#"{{"value":[
            {{"clientState":"ours","resource":"communications/presences/{me}","resourceData":{{"id":"{me}"}}}},
            {{"clientState":"ours","resource":"communications/presences('{ada}')"}},
            {{"clientState":"ours","resource":"communications/presences/{ada}","resourceData":{{"id":"{ada}"}}}},
            {{"clientState":"ours","resource":"communications/presences/not-a'guid"}}
        ]}}"#
        );
        assert_eq!(
            parse(&body, "ours"),
            vec![
                Notice::Presence(Some(me.into())),
                Notice::Presence(Some(ada.into())),
                Notice::Presence(None),
            ]
        );
    }

    #[test]
    fn the_presence_watch_starts_with_the_person_and_stops_at_the_limit() {
        let me = "aaaaaaaa-0000-4000-8000-000000000000".to_owned();
        let contacts: Vec<String> = (0..700)
            .map(|i| format!("bbbbbbbb-0000-4000-8000-{i:012x}"))
            .chain(["not a guid".into(), me.to_uppercase()])
            .collect();
        let watch = presence_watch(&me, &contacts);
        assert_eq!(watch.len(), PRESENCE_LIMIT);
        assert_eq!(watch[0], me);
        assert_eq!(watch[1], contacts[0]);
        assert!(watch.iter().all(|id| is_user_id(id)));
        assert_eq!(presence_watch(&me, std::slice::from_ref(&me)), [me]);
    }

    #[test]
    fn lifecycle_events() {
        let body = r#"{"value":[{"subscriptionId":"s1","clientState":"ours","lifecycleEvent":"reauthorizationRequired"},
            {"clientState":"ours","lifecycleEvent":"missed"},{"clientState":"ours","lifecycleEvent":"subscriptionRemoved"}]}"#;
        assert_eq!(
            parse(body, "ours"),
            vec![
                Notice::Reauthorize(Some("s1".into())),
                Notice::Missed,
                Notice::Removed(None)
            ]
        );
        assert!(parse("not json", "ours").is_empty());
    }

    #[test]
    fn one_body_yields_at_most_the_chat_notice_limit() {
        let items: Vec<String> = (0..MAX_CHAT_NOTICES + 30)
            .map(|i| {
                format!(r#"{{"clientState":"ours","resource":"chats('19:c{i}')/messages('1')"}}"#)
            })
            .chain([r#"{"clientState":"ours","lifecycleEvent":"missed"}"#.to_owned()])
            .collect();
        let body = format!(r#"{{"value":[{}]}}"#, items.join(","));
        let notices = parse(&body, "ours");
        let chats = notices
            .iter()
            .filter(|n| matches!(n, Notice::Chat(_)))
            .count();
        assert_eq!(chats, MAX_CHAT_NOTICES);
        // Lifecycle notices after the cap still count.
        assert!(notices.contains(&Notice::Missed));
    }
}
