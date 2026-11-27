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

Chaat therefore has three modes: fetch on intent, push through a relay you
run, and push through a plugin.

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

## With a relay: push updates

**Status:** implemented and tested end to end against a local relay and a
simulated Graph (`tests/push_flow.rs`). See the
[verification record](features.md#verification).

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
   dots current. Push counts as live once Graph confirms the chat
   subscription; the presence subscription follows, and Settings › Push
   updates shows it on its own line (Graph can take tens of seconds to
   confirm it, since it first checks the notification address answers).
   While connecting, the status names the step: the relay connection,
   removing the last session's subscriptions, or waiting for Graph to
   confirm chat updates. Graph allows one presence subscription per person and app,
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
   notifications. A burst of changes within a second is one fetch. Whoever
   forwards notifications sees the `clientState`, so Chaat also limits what
   they cost: at most 20 chat notices per body, and chat message fetches
   from pushes within a budget (a burst of 20, then 2 a second; over it a
   chat waits until you look, while the chat list still refreshes).
6. Chaat renews the subscription every 40 minutes, reauthorizes it when Graph
   asks (at most once every 10 minutes per subscription), recreates it when
   Graph removes it or the relay drops, and deletes
   it when push is turned off or Chaat exits, waiting up to 3 seconds for
   Graph to confirm (all accounts at once) so quitting stays quick. Lifecycle
   notices count only for a subscription Chaat holds. After a failure Chaat
   waits 5 seconds, doubling up to 5 minutes; only a session that stayed up
   2 minutes starts the wait over.
7. Chaat records its subscription ids in the account folder
   (`push-subscriptions.json`). A run that ends without deleting them (a
   crash, a killed process, Graph slower than 3 seconds) leaves them in
   Graph for up to an hour, still sending notifications to a channel nobody
   listens on, and Graph may then refuse a new presence subscription as a
   duplicate (`409`). The next run deletes them before subscribing again. It
   only deletes its own, so another device sharing the relay is unaffected.

The chat with yourself (notes, `48:notes`) is not documented by Microsoft.
A message to yourself may produce no change notification, while a
one-on-one chat does. That chat updates when the chat
list refreshes: when Chaat comes to the foreground, on ⌘R, or when another
chat's notification refreshes the list.

What the relay sees: notification envelopes with a subscription id, the
`clientState`, and resource paths such as `chats('19:…')/messages('1702')`
or `communications/presences/{id}`.
**No message content and no tokens.** It keeps nothing on disk. Anyone who
could forge a notification could only make Chaat re-fetch from Graph.

With push on, keeping Chaat running in the tray (**Settings → General**)
gives background notifications for new messages.

### Through a push plugin

A plugin that offers the `push` capability can receive Graph's notifications
instead of a hosted `chaat-relay`. The maintainer's plugin
(`chaat-push-relay`, a separate repository) runs a receiver on 127.0.0.1 and
puts it on the internet through a `cloudflared` tunnel you install yourself.
Chaat never ships or installs either. It has not yet been tried against real
Graph notifications.

How it works:

1. You add the plugin in Settings › Plugins, turn it on and allow its `push`
   need ("your update notifications: which chats changed and when, no
   message text"). Only the first enabled, running plugin that offers `push`
   is used, and only when `push` is declared and allowed.
2. Chaat makes one random channel per account for each run and never saves
   it. It sends the plugin the list of channels (`push_channels`), with no
   account IDs, after each accepted `push_ready` and again when an account is
   added or removed.
3. When the tunnel is up, the plugin sends `push_ready` with its https
   address. Chaat gives each account backend the route
   (`{url}/graph/{channel}`, `{url}/lifecycle/{channel}`). The backend
   subscribes there exactly as it does for a relay (same `clientState`,
   renewals, presence subscription and cleanup), but opens no WebSocket.
   A `push_ready` with a new address, or after a `push_down` or a plugin
   restart, subscribes again; a repeat of the same address changes nothing.
4. Graph posts to the tunnel. The plugin forwards each body as a `push` line
   on its channel. Chaat routes it to the account that owns the channel,
   checks the `clientState` as for a relay and fetches what changed with
   `Trigger::Push`. A body is one line, so it is at most 64 KiB. Chaat holds
   up to 64 bodies per account; more are dropped, and a dropped body costs
   one missed fetch, which the next look makes up.
5. When the plugin sends `push_down`, stops, is turned off or removed, or the
   need is withdrawn, Chaat stops the subscriptions and deletes them in
   Graph. It sends an empty channel list only to a plugin that is still
   running (after `push_down`, or when the need is withdrawn); a plugin that
   is stopping or stopped is sent nothing. While it is down, Chaat fetches on
   look as without push.

A relay address entered in Settings › Push updates always wins: the plugin
is sent an empty channel list and is not used. Clearing the address brings
the plugin route back if its tunnel is up.

What the plugin and Cloudflare see: notification envelopes with a
subscription id, the `clientState`, and resource paths such as
`chats('19:…')/messages('1702')`. **No message content and no tokens.** Chaat
sends the plugin only its channel names. The plugin sees what a relay would.

Settings › Push updates shows the plugin's name and one of "Tunnel at
<host>", "Waiting for the plugin's tunnel", "The plugin's tunnel is down"
(with the plugin's reason in quotation marks) or "Allow it in Settings ›
Plugins", and keeps the per-account status. With a relay address set, it
says the plugin is not used. See [plugins.md](plugins.md#push-plugins).

Its **Open** link goes to the plugin's page. If several plugins offer `push`,
the first one in the list order of Settings › Plugins (changeable with
**Reorder**) that is running provides it.

### Running the relay

```sh
cargo build --release -p chaat-relay
./target/release/chaat-relay --listen 127.0.0.1:8787
```

The listen address defaults to `127.0.0.1:8787`. `--listen ADDRESS` or the
`CHAAT_RELAY_LISTEN` environment variable sets another; the flag wins when
both are given, and an unparsable `CHAAT_RELAY_LISTEN` falls back to the
default.

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
  within the limits above, and only if the sender knows that client's random
  channel.
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

- Graph allows 10,000 Teams subscriptions per organisation. Each signed-in
  account in a running Chaat holds one chats subscription, plus one presence
  subscription while it follows presence.
- Channel posts are not subscribed yet. They would need `ChannelMessage.Read.All`
  and one subscription per channel.
- Organisations on Azure could deliver through Event Hubs instead. Not
  implemented.
