# How Chaat stays up to date

## The rule

Microsoft's Teams API overview says:

> If your app polls to see whether a resource has changed, you can only do
> that once per day. […] It's okay to GET /me/joinedTeams every time the user
> visits/refreshes your web page, but it isn't okay to GET /me/joinedTeams in
> a loop every 30 seconds to refresh that web page. Apps that don't follow
> these polling requirements will be considered in violation of the Microsoft
> APIs Terms of Use.

([source](https://learn.microsoft.com/en-us/graph/api/resources/teams-api-overview?view=graph-rest-1.0#polling-requirements))

Graph pushes changes only to infrastructure a desktop app does not have:

- a public HTTPS webhook;
- Azure Event Hubs;
- Azure Event Grid.

Chaat therefore has two modes.

## Without a relay: fetch on intent (default)

`src/sync.rs` decides every fetch. There is no timer that fetches on its own.

| Trigger | Fetches | Skipped if fetched within |
|---|---|---|
| Chaat starts / you sign in | Profile, chats, teams and channels, calendar | (always) |
| You open a chat or channel | Its messages (date-ranged when cached) | 10 s |
| You switch to Chat, Teams or Calendar | That list | 15 s / 30 min / 5 min |
| The window comes back to the foreground | The current list and the open conversation | 15 s / 10 s |
| You press ⌘R (Refresh) | The current list and the open conversation | 2 s |
| You send, edit, delete or react | The conversation, date-ranged | 1 s |
| Presence | People in the chat you open | 60 s per person |

Message refreshes ask only for what changed since the newest cached change
(`$filter=lastModifiedDateTime gt …`), as the polling rules request. Every
view shows the cache first and updates when Graph answers.

What this means in practice:

- A chat you open or return to is current.
- While Chaat sits untouched, it learns nothing new and shows no new-message
  notifications. Meeting reminders still work, because they come from the
  cached calendar.

## With a relay: push updates (experimental)

**Status:** implemented and tested end to end against a local relay and a
simulated Graph (`tests/push_flow.rs`). The maintainer confirmed real push
notifications during live-account acceptance on 17 October 2026. See the
[verification record](features.md#verification). Push remains experimental.

How it works:

1. You run `chaat-relay` (in this repository, `relay/`) on a public HTTPS
   hostname and enter its URL in **Settings → Push updates**.
2. Chaat opens a WebSocket to `{relay}/ws/{channel}`, with a random channel
   per run. If you saved an access key, it goes in the `X-Chaat-Relay-Key`
   header. A WebSocket that isn't open within 8 seconds counts as a relay
   that can't be reached, and Chaat tries again (5 seconds, then doubling,
   up to 5 minutes). While it is failing, **Retry now** in Settings → Push
   updates connects again at once. Log lines for push carry `push[…]` with the start of
   the account's id, so two accounts' lines can be told apart.
3. Chaat subscribes to Graph change notifications for all of your chat
   messages: `POST /subscriptions` on `/users/{id}/chats/getAllMessages`,
   delegated `Chat.Read` (covered by `Chat.ReadWrite`), delivered to
   `{relay}/graph/{channel}`, **without resource data**. A second
   subscription, on presence (`Presence.Read.All`), follows you, your
   one-on-one contacts and your group chats' members
   (`communications/presences?$filter=id in (…)`, up to 650 people, contacts
   first; only people in your own organisation, since people from other
   organisations get their dots when you look): your own lets the call bar follow calls, the others keep presence
   dots current. Graph allows one presence subscription per person and app,
   so Chaat replaces it when someone new appears, and a second device on the
   same account goes without. Graph refuses (400) a list that contains users
   it cannot follow and names them ("Remove the failing users and retry");
   Chaat leaves those people out and asks again, and remembers them for a
   week in the account folder (`presence-refused.json`, ids only) so later
   sessions leave them out from the first request. In practice these are
   people who left the organisation (disabled or deleted accounts); deleted
   ones, shown in chats without a name or email, are skipped up front. A 400 that
   names nobody makes Chaat search for the longest list Graph accepts. A
   conflict (409: Graph allows one presence subscription per person and app,
   and a crashed or killed run left one) makes Chaat list its own
   subscriptions for that account (`GET /subscriptions`, which with
   delegated permissions returns only this app's, for this person), delete
   the presence ones, and ask again. If that still conflicts, it tries again
   at the next 40-minute renewal; other refusals leave presence to fetches on
   look until push reconnects. Chaat assumes one device per person: a second
   device would lose its presence subscription to the first. If Graph refuses it, chat
   updates still work.
4. Graph validates the URL (the relay echoes the `validationToken`), then
   posts each change. The relay forwards it over the WebSocket.
5. Chaat checks the `clientState` secret and fetches the changed chat with the
   usual date-ranged request, plus the chat list for unread state and
   notifications. A burst of changes within a second is one fetch.
6. Chaat renews the subscription every 40 minutes, reauthorizes it when Graph
   asks, recreates it when Graph removes it or the relay drops, and deletes
   it when push is turned off or Chaat exits, waiting up to 3 seconds for
   Graph to confirm (all accounts at once) so quitting stays quick.
7. Chaat records its subscription ids in the account folder
   (`push-subscriptions.json`). A run that ends without deleting them (a
   crash, a killed process, Graph slower than 3 seconds) leaves them in
   Graph for up to an hour, still sending notifications to a channel nobody
   listens on, and Graph may then refuse a new presence subscription as a
   duplicate (`409`). The next run deletes them before subscribing again. It
   only deletes its own, so another device sharing the relay is unaffected.

The chat with yourself (notes, `48:notes`) is not documented by Microsoft.
In testing on 18 November 2026, a message to yourself produced no change
notification, while a one-on-one chat did. That chat updates when the chat
list refreshes: when Chaat comes to the foreground, on ⌘R, or when another
chat's notification refreshes the list.

What the relay sees: notification envelopes with a subscription id, the
`clientState`, and resource paths such as `chats('19:…')/messages('1702')`
or `communications/presences/{id}`.
**No message content and no tokens.** It keeps nothing on disk. Anyone who
could forge a notification could only make Chaat re-fetch from Graph.

With push on, keeping Chaat running in the tray (**Settings → General**)
gives background notifications for new messages.

### Running the relay

```sh
cargo build --release -p chaat-relay
./target/release/chaat-relay --listen 127.0.0.1:8787
```

Put it behind HTTPS on a public hostname, because Graph only posts to
`https://` URLs. For example, with [Caddy](https://caddyserver.com):

```
relay.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

A Cloudflare tunnel (`cloudflared tunnel --url http://127.0.0.1:8787`) or
any cloud load balancer works too. One relay serves many people.
Routes:

| Route | Who calls it |
|---|---|
| `GET /` | Health check |
| `GET /ws/{channel}` (WebSocket) | Chaat |
| `POST /graph/{channel}` | Microsoft Graph: change notifications and validation |
| `POST /lifecycle/{channel}` | Microsoft Graph: lifecycle notifications |

Safeguards, so one relay can serve many people:

- At most 1,000 connected clients by default (`--max-clients N` or
  `CHAAT_RELAY_MAX_CLIENTS`). Beyond that, new clients get `503`; a client
  reconnecting on its own channel is always let back in.
- Chaat pings the relay every 30 seconds. A client silent for 90 seconds is
  disconnected and its channel freed, so sleeping laptops and dropped
  networks don't hold channels.
- Clients may send messages of at most 4 KiB (they only ping). Graph's
  posts may be at most 512 KiB.

### Access key

A relay shared by an office should require an access key, so only people
you gave it to can take connection slots. Limits by IP address don't work
there: everyone behind the office network shares a few addresses.

```sh
export CHAAT_RELAY_KEY="$(openssl rand -base64 32)"
./target/release/chaat-relay --listen 127.0.0.1:8787
```

- The key comes from the environment, not an argument, because arguments
  show in process lists. It must be at least 16 visible ASCII characters;
  surrounding whitespace, such as a newline from a key file, is trimmed.
  Without it, the relay logs a warning and lets anyone connect.
- Each person enters the key under the relay URL in **Settings → Push
  updates**. Chaat keeps it in the OS credential store, never in the
  settings file, and sends it only in the `X-Chaat-Relay-Key` header of the
  WebSocket request, never in a URL, so proxies don't log it.
- A missing or wrong key gets `401`; Chaat shows "the relay needs an access
  key" or "the relay did not accept the access key".
- Graph's routes (`/graph`, `/lifecycle`) need no key: Graph cannot send one,
  and a forged notification can only make one client fetch from Graph again,
  and only if the sender knows that client's random channel.
- To change the key, restart the relay with the new one and tell everyone;
  clients with the old key are refused until they update it.

### Behind Cloudflare

A named tunnel gives the relay a stable hostname:

```sh
cloudflared tunnel create chaat-relay
cloudflared tunnel route dns chaat-relay relay.example.com
cloudflared tunnel run --protocol http2 --url http://127.0.0.1:8787 chaat-relay
```

Use `--protocol http2` (or `protocol: http2` in the tunnel's config file).
With `cloudflared`'s default, QUIC, the WebSocket of the second of two
accounts starting together stalled on every start in testing (3 of 3, 18
Nov 2026): the relay answered `101` at once, but the answer never reached
Chaat, which gave up and retried. Over HTTP/2 the same starts were clean (4
of 4). The relay's debug log (`RUST_LOG=chaat_relay=debug`) shows each
WebSocket request and its answer, without the channel or key, which tells
a stall in front of the relay from one behind it.

Cloudflare's bot and WAF rules apply to the relay's routes. In testing,
Cloudflare answered `403` to POSTs whose user agent was Python's
`urllib`, before they reached the relay, while curl's went through. Whether
a zone's rules ever challenge Microsoft Graph's own posts was not tested.
If Graph subscriptions fail validation or notifications stop arriving, add
a WAF custom rule that skips bot protection for `/graph/` and `/lifecycle/`
paths, and check **Security → Events** in Cloudflare for blocked requests.
The client WebSocket (`/ws/`) passes through tunnels; Chaat's 30-second
pings keep it within Cloudflare's idle timeout.

### Limits

- Graph allows 10,000 Teams subscriptions per organisation. Each running Chaat
  holds one.
- Channel posts are not subscribed yet. They would need `ChannelMessage.Read.All`
  and one subscription per channel.
- Organisations on Azure could deliver through Event Hubs instead. Not
  implemented.
