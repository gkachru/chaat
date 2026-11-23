# Plugins

Chaat can launch external plugins: separate programs you install and turn on
in Settings › Plugins. Chaat does not ship, download or update a plugin. A
plugin gets its data its own way and is documented in its own repository;
what Chaat knows about it is the protocol on this page. Chaat itself uses
only official, documented Microsoft APIs.

Three kinds of plugin exist today: a call-alerts plugin ([calling.md](calling.md#call-alerts-plugin)),
any program that shows presence (a status light), and a push relay plugin that
receives Graph's change notifications ([realtime.md](realtime.md#through-a-push-plugin)).
Presence display and push are covered by tests with fake plugins only, not
live.

## In Settings

Settings › Plugins has three views: the list, one plugin's page and the add
flow. Chaat's own sentences (questions, toasts, the add flow's heading) put a
plugin's name in quotation marks. The plugin's name also shows without them as
a row's label in the list and as the title of its page, and its description
shows right under that title (and under the add flow's heading); those two
sit in Chaat's layout, not in Chaat's sentences. Its option texts and
statuses show in a card under the caption "Settings · from the plugin".

### The list

**Add plugin…** (**Add…** on a narrow window) sits in the header, with
**Reorder** when there are two or more plugins. Each plugin is one row with
its name and a one-line summary in Chaat's words, such as **Off**,
**Starting…**, **Listening on 1 account**, **Tunnel at <host>**, **Needs
setup**, **Asks for <data>**, **The file changed. Review it**, **Stopped:
<reason>** or **Finish setup**. Rows that need you (a pending question,
missing required settings, a changed file, an unfinished setup, a failure)
are grouped first under **Needs you**. Clicking a row opens that plugin's
page, or its add flow if setup was never finished.

### A plugin's page

Top to bottom: **‹ Plugins** link, the plugin's name with its on/off switch,
the turn-off question when you just switched off a plugin that provides
something, its description and **About this plugin** link (HTTPS only, host
shown), a **Needs you** box for the other open questions from Chaat (changed
file, consent for needs, required settings still empty), **Status**, **Gets
from Chaat** (what Chaat sends it now: "nothing", or a list of "your accounts'
IDs and sign-in names", "your presence (availability and activity)", "your
update notifications: which chats changed and when, no message text" and "its
own settings below"), **Settings · from the plugin** (see
[Options](#options)), then **Accounts** (for a running plugin that gets
accounts: **Listening for calls**, **Waiting for sign-in** or
**Connecting…**, with **New code** while a sign-in waits), and last **File**,
folded by default, with its path, a line saying "Checked against the file you
added." or "Not checked yet." (see [File pinning](#file-pinning)) and
**Remove plugin…**. See [Consent](#consent-for-needs) for the questions.

**Turning off** a plugin that provides push or call alerts first asks, in
Chaat's words, what happens, with the plugin names in quotation marks. For
example "Turn off “Name”? “Other” provides push updates
instead.", "Push updates stop until you turn it back on or set a relay
address." or "Chaat stops showing incoming calls from this plugin." The
active agents plugin asks too: "Agents stop reading your allowed chats and
suggesting replies. Your allow lists are kept.", or "“Other” reads your
allowed chats and suggests replies instead." when another enabled agents
plugin has the `messages` need approved, followed by "N pending suggestions
are discarded." when any are waiting. Nothing is asked for a plugin that
provides none of these. While a relay address is set, no plugin counts as
providing push, so push is left out of the question.

**Remove plugin…** asks in place: "Remove “Name”? Chaat stops it, deletes its
saved secrets and tells it to forget your sign-ins.", plus the same sentence
about what stops or who takes over. **Remove** then stops it, tells a running
plugin that needs accounts to forget each sign-in, deletes its stored
secrets and drops it from the list. **Cancel** keeps it.

### Order and Reorder

Plugins are listed in the order you added them until you change it.
**Reorder** shows every plugin with a drag handle and **Move up** and **Move
down** buttons; **Done** ends it. The note there says the first plugin that
offers push or call alerts provides it. When two enabled plugins offer
`call_alerts`, the first one in the list that is running gets rings and
sign-in cards; the other is ignored and Settings says so. The same holds for
`push`: the first enabled one that is running is used. Any number of
plugins may offer `presence_display`. Moving a plugin up can therefore change
which one provides push or call alerts.

### Adding a plugin

**Add plugin…** opens the system file picker. Chaat names the plugin from
its file name, records the file's SHA-256 hash and opens the add flow. The
plugin starts with no data approved, and runs only while its add flow is
open.

1. **Starting**: "Starting the plugin…" with a spinner and **Cancel**. If the
   plugin sends no `ready` within 15 seconds, or stops first, the flow
   fails.
2. **Setup**: the plugin's name and description, "From <path>", and, under
   **It asks for**, one unticked box per need it declared. Nothing is sent
   until you tick it. Below are its settings, required ones first.
   **Done** is off until every required setting has a value, then saves
   your ticks as approvals (unticked needs are refused) and ends setup.
   **Cancel** removes the plugin.
3. **Failed**: Chaat says it stopped before saying what it does, with **Try
   again** and **Remove**. Remove deletes nothing on disk.

A setup you leave open or close Chaat on stays in the list as **Finish
setup**. Chaat does not start such a plugin at launch; clicking the row
reopens the add flow and starts it.

### Esc

On a plugin's page Esc goes back to the list. In Reorder it ends Reorder.
In the add flow it does nothing: use **Cancel** or **Done**. Elsewhere it
closes Settings.

## Transport

Chaat starts the executable as `<plugin> serve` (no console window on
Windows) and exchanges newline-delimited JSON with it: Chaat writes to the
plugin's stdin and reads its stdout. Each message is one line, an object with
`"v": 1` and a `type`. Chaat discards the plugin's stderr.

- A line over 64 KiB is dropped (logged once per run), and so is a line with
  another version.
- A message type Chaat does not know is ignored, and so are unknown fields,
  so either side can be newer. A mistyped optional field reads as
  absent. A mistyped required field (`incoming_call.id`,
  `sign_in_required.verification_uri` or `user_code`, `listening.account`)
  drops that one message.
- Plugin lines are untrusted. Text is stripped of control and invisible
  characters, collapsed and shortened. Beyond showing it (and raising the
  window), nothing happens until you click.

## How Chaat runs a plugin

- The path must be absolute. Chaat refuses a relative one, since it would
  depend on Chaat's own folder.
- The plugin runs in its own folder. Chaat removes every `CHAAT_*` variable
  from its environment; the rest is inherited.
- Everything the plugin starts dies with it: on Windows each plugin runs in a
  Job Object that kills its processes when Chaat ends the run; on Unix it
  runs in its own process group, which Chaat kills as a group. On Windows a
  short gap exists between the process starting and joining the job. The
  code is tested with fake plugins on Windows and Linux, including that
  stopping a plugin kills its children. It has not been compiled on macOS.
- **A plugin must exit when its stdin closes.** Chaat sends `shutdown`, waits
  briefly, then ends the process; if Chaat ends without stopping it, a closed
  stdin is the plugin's signal to quit.

Limits per plugin:

| Limit | Value |
|---|---|
| Lines from the plugin | Forwarded push bodies: 50 a second on average, bursts of 200. Everything else: 20 a second, bursts of 50. At most 200 lines a second are read at all. The excess is dropped and reported once per run |
| Line length | 64 KiB; longer lines are dropped |
| Messages waiting to be written to the plugin | 256; more are dropped |
| A write to the plugin | 5 seconds; a timeout ends the run |
| Plugin toasts | One every 10 seconds |

Chaat writes from its own task, so a plugin that stops reading cannot stall
Chaat's reading of its output. Each run starts with an empty outgoing queue.

## File pinning

Chaat stores the SHA-256 hash of a plugin's file in its settings and hashes
the file before every start.

- A plugin with no stored hash (just added, or from an earlier version of
  Chaat) is trusted on first use: Chaat records the hash and runs it.
- If the hash differs, Chaat does not run the file. Settings shows "The plugin
  file changed since you added it. Run the new file only if you trust it.
  It gets the settings and secrets you saved for this plugin, and asks again
  for anything else." with **Run the new file** and **Keep it off**. A
  changed file waits for you and is not retried.
- **Run the new file** stores the new hash and clears every approval and
  refusal for that plugin, so the new program must ask for its data again.
  Its option values and saved secrets stay and are sent to the new file.

Chaat hashes the file and then starts it, so a file replaced in between
would run unchecked. That gap is short and is accepted. It matters only if
someone else can write to the plugin's folder, so keep plugins in a folder
only you can write to, not a shared or downloads folder. Pinning notices a
replaced file; it does not tell you whether the original file was safe.

## Start, ready and capabilities

Chaat sends `hello` first. The plugin answers with `ready`.

`hello` carries `host: "chaat"`, `capabilities` (the capabilities Chaat
knows: `call_alerts`, `presence_display`, `push`), `accounts` and `forget`.
`accounts` is empty unless the plugin declared the `accounts` need and you
approved it (see below). `forget` lists accounts signed out of Chaat while the
plugin was off, and is sent only while accounts are approved and needed.

`ready` carries:

| Field | Meaning |
|---|---|
| `capabilities` | What the plugin does. Unknown names are ignored |
| `name`, `description`, `docs_url` | Optional, shown in Settings. `docs_url` must be HTTPS and carry no user info; Settings shows its host next to the link |
| `needs` | Optional: what it asks Chaat to share (below) |
| `options` | Optional: settings it describes (below) |
| `forgotten` | The `hello.forget` accounts whose sign-ins are now gone; send it only after clearing them |

Capabilities:

| Capability | Meaning |
|---|---|
| `call_alerts` | Reports ringing calls and sign-ins. Exclusive: only the first enabled plugin offering it, in list order, gets rings and sign-in cards |
| `presence_display` | Shows your presence. Any number of plugins |
| `push` | Receives Graph's change notifications and forwards them (see [Push plugins](#push-plugins)). Exclusive: only the first enabled plugin offering it, in list order, that is running is used, and only with the `push` need approved |
| `agents` | Lets an AI agent read cached messages and propose replies and reactions (see [Agent plugins](#agent-plugins)). Exclusive: only the first enabled plugin offering it, in list order, that is running is used, and only with the `messages` need approved |

A plugin whose `ready` offers none of the capabilities Chaat knows is
stopped with a notice and not restarted.

Each start is a generation. Chaat drops events that belong to an earlier
run, and events from a plugin that is turned off.

## Needs and least privilege

A plugin receives data only when its `ready.needs` asks for it and you have
approved that need. Without both, Chaat sends it neither accounts nor
presence.

| Need | What Chaat then sends |
|---|---|
| `accounts` | The ID (`oid.tid`) and sign-in name of each Chaat account, and later changes |
| `presence` | Your busiest presence across accounts: availability and activity |
| `push` | One random channel name per account for each run, no account IDs. The plugin sends back notification bodies Graph posted to it |
| `messages` | For the `agents` plugin only: answers to its read requests and message events, limited to the chats and channels you allowed per account (Settings › Plugins, "What agents can see"). Suggested consent text: "the text of messages in the chats and channels you allow" |

The first start sends an empty account list. Whatever the need, Chaat never
sends a plugin its tokens or its settings (except the plugin's own option
values). Without the `messages` need it never sends message content either;
with it, only what [Agent plugins](#agent-plugins) describes. Presence never
includes your status message.

`needs` limit what Chaat sends. They do not limit what the program can
reach: a plugin runs as you, with your permissions, and can read what you
can, including your files, your network and, likely, the OS credential
store that holds Chaat's tokens. Only add a plugin you trust.

### Consent for needs

Approval is stored per plugin in Chaat's settings (approved and refused
needs). When a `ready` declares a need you have neither approved nor refused,
the need is pending: Chaat shows one notice (Plugin “Name” asks for access,
open Settings › Plugins to allow it), without raising the window, and
Settings › Plugins shows `Plugin “Name” asks for:` with each pending need in
Chaat's words, and **Allow** and **Don't allow** buttons. Until you answer, nothing is sent for that
need. A need approved earlier but no longer declared in the current `ready`
is not sent either. A plugin's own option values are sent without a separate
question, and the **Receives** line lists them.

The call-alerts entry migrated from earlier Chaat settings starts with
`accounts` approved, because it already received them.

## Accounts, forget and sign-in

Chaat to plugin, for plugins that need accounts:

- `accounts`: the new account list. An account removed from Chaat stops
  listening but keeps its sign-in in the plugin.
- `forget`: clear the plugin's sign-in for this account (you signed out of
  Chaat or removed the plugin). Chaat also queues the account in its
  settings and lists it in `hello.forget` at every start, until the plugin's
  `ready.forgotten` confirms it, or the account is signed in again. Neither
  `forget` nor `hello.forget` is sent unless accounts are approved and needed.
- `sign_in`: offer a fresh code for this account now.

Plugin to Chaat:

- `sign_in_required`: `method` (`device_code`, or absent for the same),
  `verification_uri`, `user_code`, and optionally `account` and `username`.
  Chaat shows a card with the code, **Open sign-in page** and **Copy code**.
  It accepts only a code of 4 to 32 letters, digits or dashes, and only one
  of these exact pages: `https://microsoft.com/devicelogin`,
  `https://www.microsoft.com/devicelogin`,
  `https://login.microsoftonline.com/common/oauth2/deviceauth` or
  `https://login.microsoft.com/device`. The host is matched without regard to
  case; a port, user info, query, fragment, whitespace or control character
  refuses the page. Other methods and pages are refused. The card names the Chaat account (Chaat's own name for it, never
  the plugin's) and, on a line of its own, the plugin's name in quotes. Only
  an account's first code brings the window forward. Cards are keyed by
  plugin and account.
- `signed_in`: `account`, `username`.
- `listening`: `account`, `username`; sent each time an account's
  connection is up.
- `incoming_call`: for `call_alerts` plugins. See
  [calling.md](calling.md#when-a-call-rings).
- `error`: `message`, shown as an error notice that names the plugin
  (`Plugin “Name”: …`). With
  `account`, the notice names the account too and it no longer shows as
  listening.

An `account` must be the ID of one of Chaat's own accounts. Chaat drops
`sign_in_required`, `signed_in`, `listening`, `error` and `incoming_call`
that name an account it does not have. Account text from a plugin is cleaned
and shortened like other plugin text.

## Options

A plugin may describe up to 16 settings in `ready.options`. Chaat draws them
in Settings › Plugins; it never runs plugin code or renders plugin markup.
Each option has an `id`, a `kind` and a `label`. Limits: `id` is 1 to 32
characters of `a-z`, `0-9` and `_`; labels are plain text up to 60 characters.
Chaat drops an option that is malformed (bad id, label, default or limits) and
keeps the first of a repeated `id`.

| Kind | Control | Default |
|---|---|---|
| `toggle` | Checkbox | `true` or `false` |
| `text` | Text field | A string of at most 200 characters |
| `multiline` | Text box of several lines | A string of at most 2000 characters |
| `choice` | Drop-down; `choices` is 1 to 12 `{value, label}` entries (value 1 to 64 characters, unique) | One of the choice values |
| `number` | Number field | A number within `min` and `max` |
| `color` | Color field | `#rrggbb`, saved in lowercase |
| `secret` | Write-only field with **Save** and **Clear** | None |
| `action` | A button with the label; no value | None |

Optional fields on every option (plain text is stripped of control and
invisible characters, collapsed and shortened):

| Field | Meaning |
|---|---|
| `help` | Plain text, at most 200 characters, under the option |
| `placeholder` | Plain text, at most 60 characters, in an empty field |
| `section` | Plain text, at most 40 characters. Options are grouped under this heading, in the order sections first appear |
| `advanced` | `true` folds the option into an "Advanced" area of its section |
| `required` | `true` marks the option as needed for setup. A secret counts as set once saved. Ignored for `action` and unsupported kinds |
| `visible_when` | `{"option": <id>, "equals": <string or bool>}`: shown only while that other option's current value (its default if unset) equals this. A string is at most 64 characters. An id that names no option, or a secret, hides nothing. An invalid condition, or one naming the option itself, is ignored |

Fields for one kind:

| Kind | Fields |
|---|---|
| `number` | `min`, `max`, `step` (numbers; `min` at most `max`, `step` above 0), `unit` (plain, at most 12 characters). A mistyped limit drops the option |
| `text` | `format` (`ip`, `hostname`, `url_https`, `email`; any other value means no format), `min_length`, `max_length` (characters; `max_length` is capped at 200) |
| `multiline` | `min_length`, `max_length` (capped at 2000) |

A default that fails its own kind's checks drops the option. A secret or an
action with a default is dropped.

### How values are checked

Chaat checks a value against its kind before it stores it or sends it to the
plugin. A value that fails is neither saved nor sent, and the page shows the
reason under the field.

- `toggle`: `true` or `false`.
- `choice`: one of the listed values.
- `number`: a JSON number within `min` and `max` when given. Out of range is
  refused, never clamped. `step` and `unit` describe the field; Chaat does not
  round a value to the step.
- `color`: `#` and six hex digits, whitespace trimmed, saved in lowercase.
- `text`: trimmed, one line of plain text, within `min_length` and
  `max_length` (up to 200). An empty value means "not set" and passes the
  length and format checks. Formats: `ip` (IPv4 or IPv6, no port), `hostname`
  (dot-separated labels of letters, digits and hyphens, RFC 1123), `url_https`
  (an `https://` address with a host, no user info, no spaces, at most 300
  characters), `email` (one `@`, a local part of at most 64 characters without
  spaces, a host name after it).
- `multiline`: up to 2000 characters, line breaks and tabs allowed, `\r\n`
  saved as `\n`; whitespace only means "not set".
- `secret`: trimmed, plain, at most 1000 characters.

There are no regular expressions: a plugin cannot supply a pattern.

### Unsupported kinds

An option whose `kind` Chaat does not know is kept, not dropped, and Settings
shows its label with "This setting needs a newer Chaat". It has no default, is
never required and is sent no value. A value stored for it earlier is kept.
The limits of 16 options and 12 choices still apply.

### Storage and sending

Chaat stores the values of every kind except secrets in its settings file,
under the plugin's entry. Secret values never go in a file: they are stored in
the OS credential store under `plugin-option:<plugin id>:<option id>`, written
and read only by Chaat's background worker (one queue per plugin), and deleted
when you clear them or remove the plugin. Chaat sends `options { values }`
after `ready` and after each change, and only per the schema in the current
`ready`: `values` holds each described option that has a value (the stored
value, else the default) and the stored secrets of secret options. Actions and
options with neither a value nor a default are left out. A plain value stored
for an option that is now a secret is deleted from the settings file. Removing
a plugin deletes the stored secret of every secret option it described. Secret
values are never logged. An unsaved secret entry is cleared on Save, Clear, a
change of page and when Settings closes.

On each `ready`, a stored value that the option's kind, range or format now
refuses is dropped. A stored choice that is still a well-formed value is kept
even if the plugin no longer lists it. Values for options the plugin does not
describe, or of an unsupported kind, are kept.

### What you see

This describes the page from the design note ([plans/2026-11-20-plugin-options.md](plans/2026-11-20-plugin-options.md)).

- Toggles and choices save at once. Text, number and multiline values save on
  Enter (Ctrl+Enter for multiline) or when the field loses focus.
- Each section has **Reset to defaults**: options with a default get it,
  options without one lose their stored value, and secrets are untouched.
- A plugin with a required, visible option that has no value shows **Needs
  setup** and a **Set up** button on its row. It still runs, since it may need
  to run to find devices.
- After **Add plugin...**, the first `ready` that leaves required options
  unset opens that plugin's setup view: the required options first, then the
  rest.
- A status under an option is shown as the plugin's own words, in quotation
  marks: `ok` in the palette's success color, `error` in its error color and
  `working` in its dim color (with an ellipsis added unless the message
  already ends in one), so it does not pass as Chaat's text.

### Statuses, choices and actions

Plugin to Chaat:

- `option_status { id, state, message? }`: how an option or action is doing.
  `state` is `ok`, `error` or `working`; any other state drops the message.
  `id` must be an option the current `ready` described, or the status is
  ignored. `message` is plain text, at most 200 characters. The latest status
  for each option replaces the one before. Statuses are
  cleared when the plugin restarts or stops, and a status for an option that a
  new `ready` no longer describes goes.
- `options_update { id, choices }`: replaces the choices of one `choice`
  option after `ready`, with the same rules as in `ready` (at most 12, unique
  values, plain labels). A malformed list drops the message; an empty list is
  allowed. A stored value that is no longer among the choices is kept, and the
  page shows it as not available until a later update lists it again. An
  update for an id that is not a described choice option is ignored.

Chaat to plugin:

- `action { id }`: the person clicked that action's button. Chaat sends it only
  after a click, only to a running plugin that has sent `ready`, and only for
  an option the plugin described as an `action`. It never sends one on its own.
  The button shows "Working..." from the click until an `ok` or `error` status
  for the action arrives, or while its latest status is `working`, and for at
  most 30 seconds either way. What the action does is the plugin's business.

Example, a plugin that finds devices (the `ready` is one line on the wire and
is wrapped here to read):

```
plugin -> {"v":1,"type":"ready","capabilities":["presence_display"],"options":[
  {"id":"find","kind":"action","label":"Find bulbs","section":"Bulb"},
  {"id":"bulb","kind":"choice","label":"Bulb","section":"Bulb","default":"none",
   "choices":[{"value":"none","label":"None found yet"}]},
  {"id":"brightness","kind":"number","label":"Brightness","min":10,"max":100,"step":5,"unit":"%","default":80},
  {"id":"address","kind":"text","label":"Bulb address","format":"ip","advanced":true,
   "visible_when":{"option":"bulb","equals":"none"}}]}
chaat  -> {"v":1,"type":"action","id":"find"}
plugin -> {"v":1,"type":"option_status","id":"find","state":"working","message":"Listening"}
plugin -> {"v":1,"type":"options_update","id":"bulb","choices":[{"value":"b1","label":"Desk bulb"},{"value":"b2","label":"Lamp"}]}
plugin -> {"v":1,"type":"option_status","id":"find","state":"ok","message":"Found 2 bulbs"}
```

## Presence

A plugin that declared the `presence` need receives `presence
{ availability, activity }`: the busiest presence across all signed-in
accounts. Chaat sends it after `ready` and on each change, and only when
the value differs from the last one sent to that plugin.

`availability` is one of `Available`, `Busy`, `DoNotDisturb`, `BeRightBack`,
`Away`, `Offline` or `Unknown` (nothing known yet). `activity` is Graph's
activity text such as `InACall` or `InAMeeting`, and is absent when there is
none. The busiest is the highest in this order: `DoNotDisturb`, `Busy`,
`BeRightBack`, `Away`, `Available`, `Offline`; among equals, an account in a
call or meeting wins. Accounts whose presence is not known are skipped.

This is data about you leaving Chaat for a program you chose. It is sent only
to plugins that ask for it, and it never includes your status message.

## Push plugins

A plugin that offers `push` and declares the `push` need (approved in
Settings › Plugins; consent text "your update notifications: which chats
changed and when, no message text") can carry Graph's change notifications to
Chaat. [realtime.md](realtime.md#through-a-push-plugin) describes the flow.
Four messages:

| Direction | Message | Fields and rules |
|---|---|---|
| Plugin to Chaat | `push_ready` | `url`: an https origin with a host and an optional port, at most 300 characters. No user info, no path beyond `/`, no query or fragment, no whitespace, control or invisible characters. Anything else drops the message. Send it each time the address is up; Chaat subscribes again with it |
| Plugin to Chaat | `push` | `channel`: 16 to 64 characters of `A-Z a-z 0-9 - _`, one Chaat announced. `body`: the string Graph posted. A line is at most 64 KiB, which bounds the body; split a large post by its `value` entries. Unknown channels are dropped |
| Plugin to Chaat | `push_down` | `reason`: optional plain text, cleaned like other plugin text and at most 200 characters. The address no longer works; Chaat stops using it and shows the reason in quotation marks |
| Chaat to plugin | `push_channels` | `channels`: the channels the plugin may accept. Sent after each accepted `push_ready`, when an account is added or removed, and as an empty list when the route is not used (a relay address is set, the plugin went down, or the need was withdrawn). Accept requests only on listed channels |

Rules:

- Chaat treats every body as untrusted. The plugin sees the `clientState`,
  so checking it proves little; Chaat handles bodies exactly as a relay's
  (see [realtime.md](realtime.md)). It takes at most 20 chat notices from a
  body, fetches chat messages for pushes within a budget (a burst of 20, then
  2 a second; over it a chat waits until you look, while the chat list still
  refreshes), acts on lifecycle notices only for subscriptions it holds,
  reauthorizes a subscription at most once every 10 minutes, and keeps
  backing off when a recreated subscription ends within 2 minutes. A forged
  body can cause fetches within those limits and nothing else. Chaat never
  logs bodies, channels or addresses.
- Lines from the plugin follow the usual limits. A burst over them is
  dropped, and a dropped body means one missed fetch, made up the next time
  you look.
- Chaat holds up to 64 forwarded bodies per account.
- A `push_ready` from a plugin that is not the push owner, or whose need is
  not approved, changes nothing, and it is sent no channels.
- The plugin sees subscription ids, `clientState` and resource paths such as
  `chats('19:…')/messages('1702')`. Subscriptions carry no resource data, so
  there is no message text and Chaat never sends the plugin tokens. A plugin
  that tunnels through a third party (the maintainer's uses Cloudflare) shows
  that party the same.

## Agent plugins

A plugin that offers `agents` and declares the `messages` need can let an AI
agent on your computer read your cached messages and propose replies and
reactions. Chaat is the gate: it decides what the agent may see, and you
decide what gets sent. Nothing is read, answered or sent to the plugin until
all of these hold:

- the plugin is enabled, running and the first one that offers `agents`;
- you approved its `messages` need;
- the chat or channel is on that account's allow list.

The allow list is per account and closed by default. Settings › Plugins, on
the agents plugin's page, has a "What agents can see" section: for Chats and
for Channels, **None**, **Selected** or **All**, and the switch "Agents may
propose replies and reactions". The chat header menu has "Let agents read
this chat" to add or remove one conversation. Proposing a reply or reaction
needs the switch on and read access to that conversation. The list is stored
in `settings.json` (`agent_access`, by account ID).

Reads come from Chaat's local cache only. An agent request never causes a
Graph request, a fetch or a refresh, so an agent sees what Chaat already
holds, which is what you have opened or what push delivered. Conversations
are named by a key, `chat:<chat id>` or `channel:<team id>:<channel id>`,
which `list_chats` and `list_channels` return.

### Messages

| Direction | Message | Fields and rules |
|---|---|---|
| Plugin to Chaat | `agent_query` | `id`: a number the plugin picks, echoed in the answer. `account`: an account ID (`oid.tid`). `query`: an object with a `kind` (below). Unknown kinds and malformed queries are ignored without an answer |
| Plugin to Chaat | `agent_propose` | `id`, `account` as above. `proposal`: an object with a `kind` (below). Unknown kinds and malformed proposals are ignored without an answer |
| Chaat to plugin | `agent_result` | `id`: the request's. `ok`: `true` with `data`, or `false` with `data: {"error": <code>}` |
| Chaat to plugin | `agent_outcome` | `proposal`: the id from the `agent_propose` answer. `outcome`: see below |
| Chaat to plugin | `agent_event` | `account` and `event`: a new message or your own send in an allowed conversation (see below) |

Query kinds. Ids (conversation keys, message ids) are 1 to 512 characters.
`limit` is clamped to the maximum; a missing or non-numeric one means the
default. Times (`before`, `since`) are Unix milliseconds.

| `kind` | Fields | Answer `data` |
|---|---|---|
| `list_chats` | `limit` (default 50, at most 200) | `{"chats": [{id, title, kind, last_message_at, unread}]}`, newest activity first; `kind` is `one_on_one`, `group` or `meeting`; hidden chats are left out |
| `list_channels` | none | `{"channels": [{id, team, channel, last_message_at}]}`, newest first |
| `read_messages` | `conversation`, `before` (optional), `limit` (default 30, at most 200) | `{conversation_id, messages, more, before?}`, oldest first. When `more` is true, pass the returned `before` to get older messages |
| `recent_messages` | `since` (default 0), `limit` (default 50, at most 200), `unread_only` (default false) | `{"messages": [...], "more": bool}` across allowed conversations, each message also carrying `conversation_id` and `conversation` (title). `unread_only` applies to chats |
| `search` | `query` (1 to 2000 characters), `limit` (default 20, at most 100) | `{"hits": [{conversation_id, conversation, message_id, sender, time, snippet}]}` from the local search index, newest first |

A message in an answer has `message_id`, `sender`, `sender_id`, `time` (UTC,
ending in `Z`), `text` (plain text), `reply_to` and `reactions` (`emoji` and
`count`), and, when present, `subject`, `edited` and `files` (file names
only). There is no HTML, no attachment or image URL and no image data.
Deleted messages and system events are left out.

Proposal kinds:

| `kind` | Fields |
|---|---|
| `reply` | `conversation`, `text` (at most 20 000 characters; control and invisible characters are removed, newlines kept; a proposal that is empty after cleaning is dropped), `reply_to` (optional message id, for a quote reply), `agent` (the agent's name, plain text cut to 40 characters; `Agent` when missing) |
| `reaction` | `conversation`, `message` (a message id), `emoji` (1 to 16 characters, no whitespace or control characters), `agent` as above |

A proposal is shown to you and does nothing until you click. A reply appears
as a card above that conversation's composer with **Send**, **Edit** and
**Discard**; a reaction appears as a pill under the message with **Accept**
and **Discard**; the chat list marks conversations with pending proposals.
A new reply replaces the pending reply of its conversation, and a new
reaction replaces the pending one on the same message. A conversation holds
at most 20 pending reactions. Proposals live in memory only, and go when the plugin stops or is removed, its messages permission is withdrawn, or the account signs out: a restart
forgets them. Images and mentions are not supported yet, and review cannot
be turned off.

The answer to a proposal is `{"proposal_id": "p1", "state": "pending"}`. Ids
are `p1`, `p2` and so on, and are not reused in a run.

Outcomes, each sent as it happens:

| `outcome` | Meaning |
|---|---|
| `sent` | You clicked **Send** and Chaat sent the reply (it shows as your message and, if Graph refuses it, can be retried) |
| `edited` | You clicked **Edit**: the text went into the composer as a draft and the proposal is gone |
| `discarded` | You clicked **Discard**, or you took the chat off the allow list |
| `accepted` | You clicked **Accept** and Chaat reacted |
| `outdated` | You wrote in that conversation yourself. The card stays so you can discard it, but it can no longer be sent or edited as is |
| `replaced` | A newer proposal took its place |

Events arrive only for allowed conversations, as `agent_event` with an
`event` object:

| `event.type` | Fields |
|---|---|
| `new_message` | `conversation_id`, `message`: the message as in an answer. Sent for messages from others that Chaat has newly received, not for history it loads |
| `user_sent` | `conversation_id`, `message_id`: you sent a message there |

Error codes in `agent_result` (`ok: false`):

| `data.error` | Meaning |
|---|---|
| `not_allowed` | The plugin is not the active agents plugin, the conversation is not on the allow list, or the account's propose switch is off |
| `no_account` | `account` is not a signed-in account |
| `bad_request` | `conversation` is not a valid key |
| `not_found` | An allowed conversation Chaat has no record of |
| `busy` | More than 8 reads are running, the cache could not be read, or 20 reactions are already pending in that conversation |

Limits: an answer is at most 60 000 bytes. A read that does not fit returns
the newest messages that do, with `more` set, and a single message over the
budget is cut with an ellipsis. At most 8 reads run at once, across
accounts. The ordinary line limits apply.

An example, after a `ready` with `"capabilities":["agents"]` and
`"needs":["messages"]`, approved, with the chat allowed and proposing on:

```
plugin -> {"v":1,"type":"agent_query","id":7,"account":"o1.t1","query":{"kind":"read_messages","conversation":"chat:19:a@thread.v2","limit":30}}
chaat  -> {"v":1,"type":"agent_result","id":7,"ok":true,"data":{"conversation_id":"chat:19:a@thread.v2","messages":[{"message_id":"1702","sender":"Ada","sender_id":"u-ada","time":"2026-11-22T09:14:00Z","text":"Can you review the draft?","reply_to":null,"reactions":[]}],"more":false}}
plugin -> {"v":1,"type":"agent_propose","id":8,"account":"o1.t1","proposal":{"kind":"reply","conversation":"chat:19:a@thread.v2","text":"On it","agent":"Claude Code"}}
chaat  -> {"v":1,"type":"agent_result","id":8,"ok":true,"data":{"proposal_id":"p1","state":"pending"}}
chaat  -> {"v":1,"type":"agent_outcome","proposal":"p1","outcome":"sent"}
chaat  -> {"v":1,"type":"agent_event","account":"o1.t1","event":{"type":"user_sent","conversation_id":"chat:19:a@thread.v2","message_id":"1703"}}
```

What the plugin does with what it reads, including handing it to an agent
program, is outside Chaat. Read the plugin's documentation, and allow only
the conversations you are willing to show that agent.

## Restart and backoff

If a plugin exits, Chaat restarts it with backoff: 1 second, doubling to at
most 30 seconds, starting over after a run of a minute or more. Crash notices
are shown at most once per plugin every 10 minutes. Some failures are
permanent and are not retried, with one message: the file is not found,
Chaat is not allowed to run it, it is not an executable, the path is
relative, or the file changed (see [File pinning](#file-pinning)). A new
process gets a fresh `hello` (with the current accounts and the sign-out
queue, if approved) and, if it needs it, the current options and presence
again. Chaat stops a plugin when you turn it off, remove it or quit: it sends
`shutdown`, waits briefly and ends the process. Stops do not block Chaat's
other work, and quitting waits for every stop. Turning one plugin off never
stops another.

## Fixtures

Every message has an example in `tests/fixtures/plugin-protocol/`, kept
identical in each plugin's repository; tests on both sides check against
them (as JSON values; key order does not matter).

| Direction | Message | Fixture |
|---|---|---|
| Chaat to plugin | `hello` | `hello.json` |
| Chaat to plugin | `accounts` | `accounts.json` |
| Chaat to plugin | `forget` | `forget.json` |
| Chaat to plugin | `sign_in` | `sign_in.json` |
| Chaat to plugin | `options` | `options.json` |
| Chaat to plugin | `presence` | `presence.json` |
| Chaat to plugin | `action` | `action.json` |
| Chaat to plugin | `shutdown` | `shutdown.json` |
| Chaat to plugin | `push_channels` | `push_channels.json` |
| Plugin to Chaat | `ready` | `ready.json`, `ready_options.json` (with `needs` and `options`), `ready_options_rich.json` (every option kind, format, limit and `visible_when`) |
| Plugin to Chaat | `sign_in_required` | `sign_in_required.json` |
| Plugin to Chaat | `signed_in` | `signed_in.json` |
| Plugin to Chaat | `listening` | `listening.json` |
| Plugin to Chaat | `incoming_call` | `incoming_call.json` |
| Plugin to Chaat | `error` | `error.json` |
| Plugin to Chaat | `option_status` | `option_status.json` |
| Plugin to Chaat | `options_update` | `options_update.json` |
| Plugin to Chaat | `push_ready` | `push_ready.json` |
| Plugin to Chaat | `push` | `push.json` |
| Plugin to Chaat | `push_down` | `push_down.json` |
| Plugin to Chaat | `agent_query` | `agent_query.json` |
| Plugin to Chaat | `agent_propose` | `agent_propose.json` |

## A minimal exchange

A status-light plugin that needs presence and has one option:

```
chaat  -> {"v":1,"type":"hello","host":"chaat","capabilities":["call_alerts","presence_display"],"accounts":[]}
plugin -> {"v":1,"type":"ready","capabilities":["presence_display"],"name":"Status light","needs":["presence"],"options":[{"id":"ip","kind":"text","label":"Bulb address","default":"192.168.1.20"}]}
chaat  -> {"v":1,"type":"options","values":{"ip":"192.168.1.20"}}
chaat  -> {"v":1,"type":"presence","availability":"Busy","activity":"InACall"}
chaat  -> {"v":1,"type":"shutdown"}
```

## What Chaat promises

Plugins stay outside Chaat. Chaat launches only what you add and turn on,
never ships or updates one, and never gives one its tokens or its settings
(beyond the plugin's own option values). Beyond account IDs and sign-in
names, the only data it can send a plugin is your busiest presence, for a
push plugin, channel names (random, one per account, no account IDs), and,
for the one agents plugin, the text of cached messages in the chats and
channels you allowed, and only to a plugin that asked for it and that you
approved. A push plugin sends Chaat notification bodies, which Chaat checks
and treats as untrusted. An agents plugin can only propose: replies and
reactions wait for your click, and Chaat never fetches from Microsoft for it.
A file that changed since you added it does not run until you say so. These
limits cover what Chaat sends; the program itself runs as you and can read
what you can. See also [privacy.md](privacy.md).
