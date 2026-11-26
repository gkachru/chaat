//! The push loop: one session at a time, its subscriptions and the presence
//! list it follows.

use super::*;

tokio::task_local! {
    /// The account a push loop serves, for its log lines (see `push_log!`).
    pub(super) static PUSH_ACCOUNT: String;
}

/// Logs a push line tagged with the account the current push loop serves,
/// so two accounts' lines can be told apart.
macro_rules! push_log {
    ($level:ident, $($arg:tt)+) => {
        log::$level!(
            "push[{}]: {}",
            PUSH_ACCOUNT.try_with(String::clone).unwrap_or_default(),
            format_args!($($arg)+)
        )
    };
}

/// A subscription is reauthorized at most this often, however many
/// `reauthorizationRequired` notices arrive.
const REAUTHORIZE_GAP: Duration = Duration::from_secs(10 * 60);

/// A push session that stayed live this long was healthy: its end starts
/// the retry wait over.
const HEALTHY_PUSH_SESSION: Duration = Duration::from_secs(2 * 60);

/// Keeps one subscription alive while `stop` stays false: connect to the
/// relay, subscribe, forward notices, renew, and start over after failures
/// with growing pauses.
/// Where a push loop connects, for whom, and where it records the
/// subscriptions it makes.
pub(super) struct PushTarget {
    pub(super) me: String,
    pub(super) source: PushSource,
    pub(super) leftovers: std::path::PathBuf,
    /// The people of one-on-one chats, as the chat list last showed them.
    pub(super) contacts: watch::Receiver<Vec<String>>,
    /// Where people Graph refused to follow are remembered.
    pub(super) refused: std::path::PathBuf,
    /// How often the session renews its subscriptions.
    pub(super) renew_every: Duration,
    /// How long the WebSocket to the relay may take to open.
    pub(super) connect_within: Duration,
}

/// How the push loop hears from Graph.
pub(super) enum PushSource {
    /// A relay reached over a WebSocket, with its access key if it has one.
    Relay { relay: String, key: Option<String> },
    /// A push plugin: Graph posts to the plugin's tunnel and the plugin
    /// forwards each body through the queue.
    Plugin {
        route: super::super::PushRoute,
        bodies: mpsc::Receiver<String>,
    },
}

/// One thing a push session received.
enum Incoming {
    Text(String),
    Skip,
    Closed(&'static str),
}

/// People Graph refused to follow in the last week (see
/// [`realtime::REFUSED_FILE`]).
async fn load_refused(path: &std::path::Path) -> HashMap<String, i64> {
    let path = path.to_owned();
    let now = crate::time::now();
    tokio::task::spawn_blocking(move || realtime::read_refused(&path, now))
        .await
        .unwrap_or_default()
}

/// Saves newly refused people, so later sessions leave them out at once.
async fn save_refused(path: &std::path::Path, limits: &mut PresenceLimits) {
    if !limits.refused_changed {
        return;
    }
    limits.refused_changed = false;
    let path = path.to_owned();
    let refused = limits.refused.clone();
    let written =
        tokio::task::spawn_blocking(move || realtime::write_refused(&path, &refused)).await;
    if !matches!(written, Ok(Ok(()))) {
        push_log!(warn, "could not remember refused presence users");
    }
}

/// Where Graph sends a session's notifications.
struct Endpoints {
    notification: String,
    lifecycle: String,
    client_state: String,
}

/// The presence subscription and whom it follows.
struct Followed {
    id: String,
    people: Vec<String>,
}

/// The next list size to try when Graph accepted `lo` people (0: none yet)
/// and refused `hi` as malformed: halve until a size works, then bisect the
/// gap. `None` when `lo` is the most Graph takes.
fn next_presence_size(lo: usize, hi: usize) -> Option<usize> {
    if hi <= lo + 1 {
        None
    } else if lo == 0 {
        Some(hi / 2)
    } else {
        Some((lo + hi) / 2)
    }
}

/// What a push session learned about following presence, kept until push
/// reconnects so chat-list updates don't repeat refused requests.
struct PresenceLimits {
    /// The most people to ask for (0: don't try again this session).
    cap: usize,
    /// People Graph said it cannot follow (lower-case id → when), from
    /// earlier sessions too ([`realtime::REFUSED_FILE`]).
    refused: HashMap<String, i64>,
    /// Whether `refused` gained someone since it was last saved.
    refused_changed: bool,
    /// Graph answered 409 (an earlier presence subscription still holds the
    /// person's one place): try again when push next renews.
    blocked: bool,
}

impl PresenceLimits {
    fn new() -> Self {
        Self {
            cap: realtime::PRESENCE_LIMIT,
            refused: HashMap::new(),
            refused_changed: false,
            blocked: false,
        }
    }

    /// `people` without those Graph refused, up to the cap.
    fn wanted(&self, people: &[String]) -> Vec<String> {
        people
            .iter()
            .filter(|id| !self.refused.contains_key(&id.to_ascii_lowercase()))
            .take(self.cap)
            .cloned()
            .collect()
    }
}

/// How many rounds of "remove the failing users" to follow before searching.
const REFUSAL_ROUNDS: usize = 5;

/// Subscribes to the presence of `people` (the person first, then the most
/// wanted contacts). Graph refuses (400) a list containing users it cannot
/// follow and names them; those are dropped, remembered, and the rest asked
/// for again. A 400 that names nobody falls back to searching for the
/// longest accepted list (see [`search_presence`]). A conflict (409) is
/// retried once after a pause, since Graph can take a moment to forget a
/// subscription just deleted; other refusals end attempts for the session.
async fn follow_presence(
    graph: &Graph,
    people: &[String],
    to: &Endpoints,
    limits: &mut PresenceLimits,
) -> Option<Followed> {
    let mut list = limits.wanted(people);
    let mut conflict_retried = false;
    let mut rounds = 0;
    loop {
        if list.is_empty() {
            limits.cap = 0;
            return None;
        }
        match subscribe_presence_of(graph, &list, to).await {
            Ok(id) => {
                push_log!(
                    info,
                    "following presence of {} of {} people",
                    list.len(),
                    people.len()
                );
                return Some(Followed { id, people: list });
            }
            Err(GraphError::UsersRefused(ids)) if rounds < REFUSAL_ROUNDS => {
                rounds += 1;
                push_log!(
                    info,
                    "Graph cannot follow {} people; leaving them out",
                    ids.len()
                );
                let now = crate::time::now();
                limits
                    .refused
                    .extend(ids.iter().map(|id| (id.to_ascii_lowercase(), now)));
                limits.refused_changed = true;
                list = limits.wanted(people);
            }
            Err(GraphError::UsersRefused(_) | GraphError::Http { status: 400, .. }) => {
                return search_presence(graph, &list, people.len(), to, limits).await;
            }
            Err(GraphError::Http { status: 409, .. }) if !conflict_retried => {
                conflict_retried = true;
                let cleared = clear_presence_blockers(graph).await;
                push_log!(
                    info,
                    "presence subscription conflict (409); deleted {cleared} in the way, retrying"
                );
                if cleared == 0 {
                    // Graph may still be forgetting one just deleted.
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
            Err(GraphError::Http { status: 409, .. }) => {
                push_log!(
                    info,
                    "presence subscription conflict (409); trying again at the next renewal"
                );
                limits.cap = 0;
                limits.blocked = true;
                return None;
            }
            Err(error) => {
                push_log!(
                    info,
                    "no presence subscription for {} people ({error})",
                    list.len()
                );
                limits.cap = 0;
                return None;
            }
        }
    }
}

/// Deletes this app's presence subscriptions for the person, which hold the
/// one place Graph allows (a run that crashed or was killed before cleaning
/// up). Callers hold none of their own when they ask. Chaat runs on one
/// device per person; a second one would lose its presence subscription
/// here and fall back to fetching on look. Returns how many were deleted.
async fn clear_presence_blockers(graph: &Graph) -> usize {
    let Ok(subscriptions) = graph.subscriptions().await else {
        return 0;
    };
    let mut deleted = 0;
    for subscription in subscriptions.iter().filter(|s| {
        s.resource.as_deref().is_some_and(|resource| {
            resource
                .to_ascii_lowercase()
                .contains("communications/presences")
        })
    }) {
        if graph.delete_subscription(&subscription.id).await.is_ok() {
            deleted += 1;
        }
    }
    deleted
}

async fn subscribe_presence_of(
    graph: &Graph,
    people: &[String],
    to: &Endpoints,
) -> Result<String, GraphError> {
    graph
        .subscribe_presence(
            people,
            &to.notification,
            &to.lifecycle,
            &to.client_state,
            realtime::LIFETIME,
        )
        .await
        .map(|subscription| subscription.id)
}

/// Finds the longest prefix of `people` Graph accepts when it refuses a list
/// without saying whom it objects to: halving until a size works, then
/// bisecting. Graph allows one presence subscription per person and app, so
/// trying a longer list first deletes the one that works, and a refusal
/// recreates it. Keeps the size found in `limits.cap` for the session.
async fn search_presence(
    graph: &Graph,
    people: &[String],
    asked: usize,
    to: &Endpoints,
    limits: &mut PresenceLimits,
) -> Option<Followed> {
    let total = people.len();
    let mut lo = 0;
    let mut hi = total;
    let mut held: Option<Followed> = None;
    let mut size = total / 2;
    while size > 0 {
        if let Some(old) = held.take() {
            let _ = graph.delete_subscription(&old.id).await;
        }
        match subscribe_presence_of(graph, &people[..size], to).await {
            Ok(id) => {
                lo = size;
                held = Some(Followed {
                    id,
                    people: people[..size].to_vec(),
                });
            }
            Err(GraphError::UsersRefused(_) | GraphError::Http { status: 400, .. }) => {
                push_log!(
                    info,
                    "presence subscription for {size} people refused (400)"
                );
                hi = size;
            }
            Err(error) => {
                push_log!(
                    info,
                    "presence subscription for {size} people failed ({error})"
                );
                hi = size;
                if lo == 0 {
                    break;
                }
            }
        }
        match next_presence_size(lo, hi) {
            Some(next) => size = next,
            None => break,
        }
    }
    if lo == 0 {
        limits.cap = 0;
        return None;
    }
    if held.is_none() {
        // The last, longer try was refused after the working one was deleted.
        match subscribe_presence_of(graph, &people[..lo], to).await {
            Ok(id) => {
                held = Some(Followed {
                    id,
                    people: people[..lo].to_vec(),
                })
            }
            Err(error) => {
                push_log!(info, "no presence subscription for {lo} people ({error})");
                limits.cap = 0;
                return None;
            }
        }
    }
    push_log!(info, "following presence of {lo} of {asked} people");
    limits.cap = lo;
    held
}

/// The ids of the subscriptions a session holds.
/// What following presence came to, for the person.
fn presence_follow(presence: Option<&Followed>) -> PresenceFollow {
    presence.map_or(PresenceFollow::NotFollowed, |p| {
        PresenceFollow::Following(p.people.len())
    })
}

fn held(chat: &str, presence: Option<&Followed>) -> Vec<String> {
    std::iter::once(chat.to_owned())
        .chain(presence.map(|p| p.id.clone()))
        .collect()
}

/// Records the subscriptions a session holds, so a run that ends without
/// cleaning up can be cleaned up by the next one.
async fn record_subscriptions(path: &std::path::Path, ids: &[String]) {
    let path = path.to_owned();
    let ids = ids.to_vec();
    let written = tokio::task::spawn_blocking(move || realtime::write_leftovers(&path, &ids)).await;
    if !matches!(written, Ok(Ok(()))) {
        push_log!(warn, "could not record the subscriptions");
    }
}

/// Deletes subscriptions an earlier run made and never deleted. Best effort:
/// any that remain expire within the hour.
async fn delete_leftovers(graph: &Graph, path: &std::path::Path) {
    let read = path.to_owned();
    let ids = tokio::task::spawn_blocking(move || realtime::read_leftovers(&read))
        .await
        .unwrap_or_default();
    if ids.is_empty() {
        return;
    }
    let mut deleted = 0;
    for id in &ids {
        if graph.delete_subscription(id).await.is_ok() {
            deleted += 1;
        }
    }
    push_log!(
        info,
        "deleted {deleted} of {} subscriptions left by an earlier run",
        ids.len()
    );
    record_subscriptions(path, &[]).await;
}

pub(super) async fn push_loop(
    graph: Arc<Graph>,
    target: PushTarget,
    out: Outbox,
    internal: mpsc::UnboundedSender<Internal>,
    mut stop: watch::Receiver<bool>,
) {
    use futures_util::{SinkExt, StreamExt};
    let PushTarget {
        me,
        source,
        leftovers,
        mut contacts,
        refused,
        renew_every,
        connect_within,
    } = target;
    let (relay_key, mut bodies, relay, fixed_channel) = match source {
        PushSource::Relay { relay, key } => (Some(key), None, relay, None),
        PushSource::Plugin { route, bodies } => {
            (None, Some(bodies), route.url, Some(route.channel))
        }
    };
    let mut pause = Duration::from_secs(5);
    while !*stop.borrow() {
        out.send(AccountEvent::Push(PushStatus::Connecting(
            if relay_key.is_some() {
                PushStep::Relay
            } else {
                PushStep::Cleanup
            },
        )));
        let channel = fixed_channel
            .clone()
            .unwrap_or_else(|| auth::random_token(32));
        let client_state = auth::random_token(24);
        let mut live_at: Option<Instant> = None;
        let failure = 'session: {
            let mut sink = None;
            let mut stream = None;
            if let Some(key) = &relay_key {
                let Some(request) = realtime::websocket_request(&relay, &channel, key.as_deref())
                else {
                    break 'session "the relay address or access key is not valid".to_owned();
                };
                let connecting =
                    tokio::time::timeout(connect_within, tokio_tungstenite::connect_async(request))
                        .await;
                let Ok(connected) = connecting else {
                    break 'session "the relay did not answer in time".to_owned();
                };
                let socket = match connected {
                    Ok((socket, _)) => socket,
                    Err(tokio_tungstenite::tungstenite::Error::Http(response))
                        if response.status() == 401 =>
                    {
                        break 'session if key.is_some() {
                            "the relay did not accept the access key".to_owned()
                        } else {
                            "the relay needs an access key".to_owned()
                        };
                    }
                    Err(_) => break 'session "the relay could not be reached".to_owned(),
                };
                let (write, read) = socket.split();
                sink = Some(write);
                stream = Some(read);
            }
            if relay_key.is_some() {
                out.send(AccountEvent::Push(PushStatus::Connecting(
                    PushStep::Cleanup,
                )));
            }
            delete_leftovers(&graph, &leftovers).await;
            out.send(AccountEvent::Push(PushStatus::Connecting(PushStep::Chats)));
            let subscription = match graph
                .subscribe_chat_messages(
                    &me,
                    &realtime::notification_url(&relay, &channel),
                    &realtime::lifecycle_url(&relay, &channel),
                    &client_state,
                    realtime::LIFETIME,
                )
                .await
            {
                Ok(subscription) => subscription,
                Err(error) => {
                    break 'session format!("Microsoft Graph refused the subscription ({error})");
                }
            };
            // Chat updates arrive from here on; presence follows.
            out.send(AccountEvent::Push(PushStatus::Live));
            out.send(AccountEvent::PushPresence(PresenceFollow::Connecting));
            live_at = Some(Instant::now());
            // Presence: the person's own, so the call bar follows calls, and
            // their one-on-one contacts', for the dots. Optional: chat updates
            // still work without it.
            let endpoints = Endpoints {
                notification: realtime::notification_url(&relay, &channel),
                lifecycle: realtime::lifecycle_url(&relay, &channel),
                client_state: client_state.clone(),
            };
            let wanted = realtime::presence_watch(&me, &contacts.borrow_and_update());
            let mut limits = PresenceLimits::new();
            limits.refused = load_refused(&refused).await;
            let mut presence = follow_presence(&graph, &wanted, &endpoints, &mut limits).await;
            out.send(AccountEvent::PushPresence(presence_follow(
                presence.as_ref(),
            )));
            save_refused(&refused, &mut limits).await;
            let mut subscriptions = held(&subscription.id, presence.as_ref());
            record_subscriptions(&leftovers, &subscriptions).await;
            let mut contacts_open = true;
            let mut reauthorized: HashMap<String, Instant> = HashMap::new();
            // A change may have happened while connecting.
            let _ = internal.send(Internal::PushMissed);
            let mut renew = tokio::time::interval(renew_every);
            renew.tick().await;
            let mut keepalive = tokio::time::interval(Duration::from_secs(30));
            keepalive.tick().await;
            let reason = loop {
                tokio::select! {
                    changed = stop.changed() => {
                        if changed.is_err() || *stop.borrow() {
                            break None;
                        }
                    }
                    changed = contacts.changed(), if contacts_open => {
                        if changed.is_err() {
                            contacts_open = false;
                            continue;
                        }
                        let wanted = realtime::presence_watch(&me, &contacts.borrow_and_update());
                        let wanted = limits.wanted(&wanted);
                        let followed = presence.as_ref().map_or(&[][..], |p| p.people.as_slice());
                        // Graph cannot change whom a subscription follows,
                        // and allows one presence subscription per person
                        // and app: a new contact means replacing it.
                        if wanted.iter().any(|id| !followed.contains(id)) {
                            if let Some(old) = presence.take() {
                                let _ = graph.delete_subscription(&old.id).await;
                            }
                            presence = follow_presence(&graph, &wanted, &endpoints, &mut limits).await;
                            out.send(AccountEvent::PushPresence(presence_follow(presence.as_ref())));
                            save_refused(&refused, &mut limits).await;
                            subscriptions = held(&subscription.id, presence.as_ref());
                            record_subscriptions(&leftovers, &subscriptions).await;
                        }
                    }
                    _ = renew.tick() => {
                        let mut failed = None;
                        for id in &subscriptions {
                            if let Err(error) = graph.renew_subscription(id, realtime::LIFETIME).await {
                                failed = Some(format!("renewing the subscription failed ({error})"));
                            }
                        }
                        if failed.is_some() {
                            break failed;
                        }
                        // A presence subscription Graph refused as a
                        // duplicate gets one more try now; the one in the
                        // way, if abandoned, expires within the hour.
                        if presence.is_none() && limits.blocked {
                            limits.blocked = false;
                            limits.cap = realtime::PRESENCE_LIMIT;
                            let wanted = realtime::presence_watch(&me, &contacts.borrow());
                            presence = follow_presence(&graph, &wanted, &endpoints, &mut limits).await;
                            out.send(AccountEvent::PushPresence(presence_follow(presence.as_ref())));
                            save_refused(&refused, &mut limits).await;
                            subscriptions = held(&subscription.id, presence.as_ref());
                            record_subscriptions(&leftovers, &subscriptions).await;
                        }
                    }
                    _ = keepalive.tick() => {
                        if let Some(sink) = sink.as_mut()
                            && sink.send(tokio_tungstenite::tungstenite::Message::Ping(Vec::new().into())).await.is_err()
                        {
                            break Some("the relay connection dropped".to_owned());
                        }
                    }
                    incoming = async {
                        if let Some(stream) = stream.as_mut() {
                            match stream.next().await {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => Incoming::Text(text.to_string()),
                                Some(Ok(_)) => Incoming::Skip,
                                Some(Err(_)) | None => Incoming::Closed("the relay connection dropped"),
                            }
                        } else if let Some(bodies) = bodies.as_mut() {
                            match bodies.recv().await {
                                Some(text) => Incoming::Text(text),
                                None => Incoming::Closed("the push plugin stopped"),
                            }
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        let text = match incoming {
                            Incoming::Text(text) => text,
                            Incoming::Skip => continue,
                            Incoming::Closed(reason) => break Some(reason.to_owned()),
                        };
                        let mut recreate = false;
                        for notice in realtime::parse(&text, &client_state) {
                            match notice {
                                Notice::Chat(chat) => {
                                    push_log!(debug,"chat change notice");
                                    let _ = internal.send(Internal::Pushed(chat));
                                }
                                Notice::Presence(user) => {
                                    let _ = internal.send(Internal::PresencePushed(user));
                                }
                                Notice::Missed => {
                                    let _ = internal.send(Internal::PushMissed);
                                }
                                // Lifecycle notices count only for a subscription
                                // this session holds, and a reauthorization at
                                // most once per `REAUTHORIZE_GAP`: whoever
                                // forwards bodies knows `clientState`.
                                Notice::Reauthorize(Some(id)) if subscriptions.contains(&id) => {
                                    let now = Instant::now();
                                    let recent = reauthorized
                                        .get(&id)
                                        .is_some_and(|at| now.saturating_duration_since(*at) < REAUTHORIZE_GAP);
                                    if !recent {
                                        reauthorized.insert(id.clone(), now);
                                        if graph.reauthorize_subscription(&id).await.is_err() {
                                            recreate = true;
                                        }
                                    }
                                }
                                Notice::Removed(Some(id)) if subscriptions.contains(&id) => {
                                    recreate = true;
                                }
                                Notice::Reauthorize(_) | Notice::Removed(_) => {
                                    push_log!(debug, "lifecycle notice for a subscription not held; ignored");
                                }
                            }
                        }
                        if recreate {
                            break Some("Microsoft Graph ended the subscription".to_owned());
                        }
                    }
                }
            };
            // Both at once, to keep quitting quick. A failure stays listed
            // for the next start; anything left expires within the hour.
            let deletes = subscriptions.iter().map(|id| graph.delete_subscription(id));
            let results = futures_util::future::join_all(deletes).await;
            let remaining: Vec<String> = subscriptions
                .iter()
                .zip(&results)
                .filter(|(_, result)| result.is_err())
                .map(|(id, _)| id.clone())
                .collect();
            record_subscriptions(&leftovers, &remaining).await;
            match reason {
                Some(reason) => reason,
                None => return,
            }
        };
        if *stop.borrow() {
            return;
        }
        // Only a session that stayed up a while starts the wait over: one
        // ended at once (say, by a `subscriptionRemoved` again and again)
        // keeps backing off, so subscriptions are not remade every few
        // seconds.
        if live_at.is_some_and(|at| at.elapsed() >= HEALTHY_PUSH_SESSION) {
            pause = Duration::from_secs(5);
        }
        push_log!(info, "{failure}; retrying in {pause:?}");
        out.send(AccountEvent::Push(PushStatus::Failed(failure)));
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            _ = stop.changed() => {}
        }
        pause = (pause * 2).min(Duration::from_secs(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presence_search_finds_the_longest_accepted_list() {
        let total = 66;
        for limit in 1..=total {
            let (mut lo, mut hi, mut size, mut tries) = (0, total + 1, total, 0);
            loop {
                tries += 1;
                if size <= limit {
                    lo = size;
                } else {
                    hi = size;
                }
                match next_presence_size(lo, hi) {
                    Some(next) => size = next,
                    None => break,
                }
            }
            assert_eq!(lo, limit, "limit {limit}");
            assert!(tries <= 13, "limit {limit} took {tries} tries");
        }
        // Everyone accepted at once: one try.
        assert_eq!(next_presence_size(66, 67), None);
    }
}
