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
- **Turn on** and **Remove**;
- the options it describes (see [Options](#options));
- for a plugin that needs accounts, one row per Chaat account: **Listening**,
  **Waiting for sign-in** or **Not signed in**, with a **Sign in** button.

**Add plugin…** opens the system file picker. Chaat names the plugin from its
file name and turns it on. **Remove** stops it, tells a running plugin that
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

- A line over 64 KiB is dropped, and so is a line with another version.
- A message type Chaat does not know is ignored, and so are unknown fields,
  so either side can be newer. A field of the wrong type reads as absent
  rather than dropping the message.
- Plugin lines are untrusted. Text is stripped of control and invisible
  characters, collapsed and shortened. Beyond showing it (and raising the
  window), nothing happens until you click.

## Start, ready and capabilities

Chaat sends `hello` first. The plugin answers with `ready`.

`hello` carries `host: "chaat"`, `capabilities` (the capabilities Chaat
knows: `call_alerts`, `presence_display`), `accounts` and `forget`.
`accounts` is empty unless the plugin declared the `accounts` need (see
below). `forget` lists accounts signed out of Chaat while the plugin was off.

`ready` carries:

| Field | Meaning |
|---|---|
| `capabilities` | What the plugin does. Unknown names are ignored |
| `name`, `description`, `docs_url` | Optional, shown in Settings. `docs_url` must be HTTPS |
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

## Needs and least privilege

A plugin receives data only when its `ready.needs` asks for it. Without a
need, Chaat sends it neither accounts nor presence.

| Need | What Chaat then sends |
|---|---|
| `accounts` | The ID (`oid.tid`) and sign-in name of each Chaat account, and later changes |
| `presence` | Your busiest presence across accounts: availability and activity |

The first start sends an empty account list. Once a `ready` asks for
`accounts`, Chaat sends them and remembers the need in its settings, so
later `hello` messages carry them.

Whatever the need, Chaat never sends a plugin its tokens, its settings
(except the plugin's own option values) or any message content. Presence
never includes your status message.

## Accounts, forget and sign-in

Chaat to plugin, for plugins that need accounts:

- `accounts`: the new account list. An account removed from Chaat stops
  listening but keeps its sign-in in the plugin.
- `forget`: clear the plugin's sign-in for this account (you signed out of
  Chaat or removed the plugin). Chaat also queues the account in its
  settings and lists it in `hello.forget` at every start, until the plugin's
  `ready.forgotten` confirms it, or the account is signed in again.
- `sign_in`: offer a fresh code for this account now.

Plugin to Chaat:

- `sign_in_required`: `method` (`device_code`, or absent for the same),
  `verification_uri`, `user_code`, and optionally `account` and `username`.
  Chaat shows a card with the code, **Open sign-in page** and **Copy code**.
  It accepts only a code of 4 to 32 letters, digits or dashes, entered on an
  HTTPS page on Microsoft's sign-in hosts. Other methods and pages are
  refused. The card names the Chaat account (Chaat's own name for it, never
  the plugin's) and, on a line of its own, the plugin's name in quotes. Only
  an account's first code brings the window forward. Cards are keyed by
  plugin and account.
- `signed_in`: `account`, `username`.
- `listening`: `account`, `username`; sent each time an account's
  connection is up.
- `incoming_call`: for `call_alerts` plugins. See
  [calling.md](calling.md#when-a-call-rings).
- `error`: `message`, shown as an error notice that names the plugin. With
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
id>`, written and read only by Chaat's background worker, and deleted when
you clear them or remove the plugin. Chaat sends `options { values }` after
`ready` and after each change; `values` holds every option (an unset toggle,
text or choice option carries its default) and the secrets that are stored.
Secret values are never logged.

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

If a plugin exits, or cannot start, Chaat restarts it with backoff: 1 second,
doubling to at most 30 seconds, starting over after a run of a minute or
more. One notice per run of failures. A new process gets a fresh `hello`
(with the current accounts and the sign-out queue) and, if it needs it, the
current options and presence again. Chaat stops a plugin when you turn it
off, remove it or quit: it sends `shutdown`, waits briefly and ends the
process. Turning one plugin off never stops another.

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
presence, and only to a plugin that asked for it. See also
[privacy.md](privacy.md).
