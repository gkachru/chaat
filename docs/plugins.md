# Plugins

Chaat can launch external plugins: separate programs you install and turn on
in Settings › Plugins. Chaat does not ship, download or update a plugin. A
plugin gets its data its own way and is documented in its own repository;
what Chaat knows about it is the protocol on this page. Chaat itself uses
only official, documented Microsoft APIs.

Two plugins exist today: a call-alerts plugin ([calling.md](calling.md#call-alerts-plugin))
and any program that shows presence (a status light). Presence display is
covered by tests with fake plugins only, not live.

## In Settings

Settings › Plugins lists the plugins you added. Under Chaat's fixed line that
a plugin is a separate program, each one shows:

- its own name and description, as plain shortened text, and an **About this
  plugin** link when it gives an HTTPS address;
- its file path and a status: **Off**, **Running**, **Not running** (with
  the reason after a failure);
- a **Receives** line saying what Chaat sends it now: "nothing", or a list
  of "your accounts' IDs and sign-in names", "your presence (availability
  and activity)" and "its own settings below";
- **Turn on** and **Remove**;
- a question from Chaat when the plugin asks for data you have not decided
  on, or when its file changed (see [Consent](#consent-for-needs) and
  [File pinning](#file-pinning));
- the options it describes (see [Options](#options));
- for a plugin that needs accounts, one row per Chaat account: **Listening for calls**,
  **Waiting for sign-in** or **Connecting…**, with a **New code** button
  while a sign-in waits.

**Add plugin…** opens the system file picker. Chaat names the plugin from its
file name, records the file's SHA-256 hash and turns it on. A plugin starts
with no data approved. **Remove** stops it, tells a running plugin that
needs accounts to forget each sign-in, deletes its stored secrets, and drops
it from the list.

Plugins are listed in the order you added them. When two enabled plugins
offer `call_alerts`, the first one in the list that is running gets rings and
sign-in cards; the other is ignored and Settings says so. Any number of
plugins may offer `presence_display`.

## Transport

Chaat starts the executable as `<plugin> serve` (no console window on
Windows) and exchanges newline-delimited JSON with it: Chaat writes to the
plugin's stdin and reads its stdout. Each message is one line, an object with
`"v": 1` and a `type`. Chaat discards the plugin's stderr.

- A line over 64 KiB is dropped (logged once per run), and so is a line with
  another version.
- A message type Chaat does not know is ignored, and so are unknown fields,
  so either side can be newer. In every message, a field of the wrong type
  reads as absent rather than dropping the message.
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
  Windows code is tested with fake plugins; the Unix process-group code is
  compiled but has not been run.
- **A plugin must exit when its stdin closes.** Chaat sends `shutdown`, waits
  briefly, then ends the process; if Chaat ends without stopping it, a closed
  stdin is the plugin's signal to quit.

Limits per plugin:

| Limit | Value |
|---|---|
| Lines from the plugin | 20 a second on average, bursts of 50; the excess is dropped and reported once per run |
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
  file changed since you added it. Run the new file only if you trust it."
  with **Run the new file** and **Keep it off**. A changed file waits for you and is not retried.
- **Run the new file** stores the new hash and clears every approval and
  refusal for that plugin, so the new program must ask for its data again.

Chaat hashes the file and then starts it, so a file replaced in between
would run unchecked. That gap is short and is accepted. Pinning notices a
replaced file; it does not tell you whether the original file was safe.

## Start, ready and capabilities

Chaat sends `hello` first. The plugin answers with `ready`.

`hello` carries `host: "chaat"`, `capabilities` (the capabilities Chaat
knows: `call_alerts`, `presence_display`), `accounts` and `forget`.
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

The first start sends an empty account list. Whatever the need, Chaat never
sends a plugin its tokens, its settings (except the plugin's own option
values) or any message content. Presence never includes your status message.

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

An `account` must be the ID of one of Chaat's own accounts. Chaat drops
`sign_in_required`, `signed_in`, `listening`, `error` and `incoming_call`
that name an account it does not have. Account text from a plugin is cleaned
and shortened like other plugin text.
- `listening`: `account`, `username`; sent each time an account's
  connection is up.
- `incoming_call`: for `call_alerts` plugins. See
  [calling.md](calling.md#when-a-call-rings).
- `error`: `message`, shown as an error notice that names the plugin
  (`Plugin “Name”: …`). With
  `account`, the notice names the account too and it no longer shows as
  listening.

## Options

A plugin may describe up to 16 settings in `ready.options`. Chaat draws them
in Settings › Plugins; it never runs plugin code or renders plugin markup.
Each option has an `id`, a `kind`, a `label` and, for some kinds, a default:

| Kind | Control | Default |
|---|---|---|
| `toggle` | Checkbox | `true` or `false` |
| `text` | Text field | A string of at most 200 characters |
| `choice` | Drop-down; `choices` is 1 to 12 `{value, label}` entries | One of the choice values |
| `secret` | Write-only field with **Save** and **Clear** | None |

Limits: `id` is 1 to 32 characters of `a-z`, `0-9` and `_`; labels are plain
text up to 60 characters; a choice value is 1 to 64 characters. Chaat drops
an option that is malformed and keeps the first of a repeated `id`.

Chaat stores the values of toggle, text and choice options in its settings
file, under the plugin's entry. Secret values never go in a file: they are
stored in the OS credential store under `plugin-option:<plugin id>:<option
id>`, written and read only by Chaat's background worker (one queue per plugin),
and deleted when you clear them or remove the plugin. Chaat sends `options { values }` after
`ready` and after each change, and only per the schema in the current `ready`:
`values` holds each option that `ready` describes (an unset toggle, text or
choice option carries its default) and the stored secrets of secret options.
A plain value stored for an option that is now a secret is deleted from the
settings file. Removing a plugin deletes the stored secret of every secret
option it described. Secret values are never logged. An unsaved secret entry
is cleared on Save, Clear, a change of page and when Settings closes.

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
| Chaat to plugin | `shutdown` | `shutdown.json` |
| Plugin to Chaat | `ready` | `ready.json`, `ready_options.json` (with `needs` and `options`) |
| Plugin to Chaat | `sign_in_required` | `sign_in_required.json` |
| Plugin to Chaat | `signed_in` | `signed_in.json` |
| Plugin to Chaat | `listening` | `listening.json` |
| Plugin to Chaat | `incoming_call` | `incoming_call.json` |
| Plugin to Chaat | `error` | `error.json` |

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
never ships or updates one, and never gives one its tokens, its settings
(beyond the plugin's own option values) or your messages. Beyond account IDs
and sign-in names, the only data it can send a plugin is your busiest
presence, and only to a plugin that asked for it and that you approved. A
file that changed since you added it does not run until you say so. These
limits cover what Chaat sends; the program itself runs as you and can read
what you can. See also [privacy.md](privacy.md).
