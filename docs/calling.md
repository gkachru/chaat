# Calling and meetings

Chaat opens calls in Microsoft Teams on the web and stays beside them for
chat and call status. Audio, video, screen sharing and call controls stay in
Teams. Chaat does not request microphone, camera or screen-recording access.
Graph cannot tell Chaat that a call is ringing; an optional, separately
installed [call-alerts plugin](#call-alerts-plugin) can.

The implementation has mock, headless UI and macOS screenshot coverage.
A real call has not yet been confirmed; see [open questions](#open-questions).

## Opening a call

Select **Call** in a chat, **Join** on a meeting, or paste a Teams meeting
link in the Calls page. By default, Chaat opens an Edge or Chrome app window
without browser tabs. If neither browser is installed, it uses the default
browser. The Teams desktop app is an explicit choice in Settings.

The **Calls and meetings** page in Settings controls:

| Setting | Behavior |
|---|---|
| Open in | A Teams web app window, the default browser, or the Teams app |
| Browser | An installed Edge or Chrome; Edge is preferred by default |
| Open all links in Edge or Chrome | Links in messages, the calendar and "Open in Teams" open in the chosen browser and the same profile as calls, instead of the default browser; off by default. `mailto:` links still use the default mail app. Shown whenever Edge or Chrome is installed |
| Move Chaat beside the call window during calls | Dock Chaat in a 380-point panel and restore its previous geometry afterward; on by default |
| A separate browser profile for each account | Optional, shown with multiple accounts; off by default |
| Signing out of Chaat also signs the browser out of Microsoft | Optional; off by default |
| Sign in to Teams on the web | Open Teams once to complete browser sign-in and choose whether to stay signed in |
| Call alerts | A line and an **Open Plugins** button; call alerts come from a plugin you add in Settings › Plugins. None by default; see [below](#call-alerts-plugin) |

Docking needs a window at least 960 × 400 points. It is unavailable on
Wayland and beside the Teams desktop app. Browser window placement is a
request; the browser may choose a different position.

## During a call

The call bar offers **Meeting chat**, **Join on phone**, **Dock/Restore**,
and **Done**. **Show call** activates the browser on macOS. Joining the same
meeting again brings the current call forward.

Chaat finds the meeting chat in its cache. If it is missing, **Meeting chat**
requests `OnlineMeetings.Read` and looks it up through Graph. This permission
is requested only when needed. Calls started from a chat use that chat.

Call status follows your presence on focus and, with a relay, through
presence change notifications. These trigger ordinary Graph fetches; Chaat
does not poll. After presence leaves a call, a one-minute grace period
avoids flicker before the window is restored. **Done** restores it at once.

Calendar-derived `InAMeeting` counts only for meetings Chaat opened. It can
outlast a meeting you leave early; use **Done** if the bar stays visible.
Calls started elsewhere can also appear when your presence indicates a call.

## Sign-in and privacy

Chaat opens its Microsoft sign-in in the selected call browser and profile.
Teams on the web then uses that browser's Microsoft session. Tenant policy
and multiple signed-in accounts can still require an account picker or a
new sign-in. Chaat never passes its tokens to Teams or reads browser cookies.

Separate browser profiles keep account sessions apart. Optional browser
sign-out opens Microsoft's logout page, which can sign that browser out of
other Microsoft sites too.

Only HTTPS links on Teams hosts are opened, using process arguments without
a shell. Join links and browser command lines are not logged.

## Call-alerts plugin

Microsoft Graph has no delegated way to learn that a call is ringing: change
notifications arrive only after the call, and Graph calling is for bots. A
call-alerts plugin can supply that signal. It is a separate program the
person installs and turns on; Chaat does not ship, download or update one.
How a plugin detects calls, how it signs in and what that means for your
organization's policy are the plugin's own business and documented with it.
For `call-alerts`, which uses Teams' own push service and sign-in
outside Microsoft's documented APIs, see its
[README](https://github.com/gkachru/call-alerts#readme).

What Chaat knows about a plugin is the protocol in [plugins.md](plugins.md):
it starts the program as `<plugin> serve` and exchanges JSON lines with it.
Identities are Entra object and tenant IDs and sign-in names. Chaat's code
holds no Teams formats and no assumptions about a plugin's command line
beyond `serve`. This section describes the call-alerts side of it.

### Turning it on

Settings › Plugins › **Add plugin…** picks the executable and turns it on
(the Calls and meetings page has an **Open Plugins** button for it). Chaat runs it on its own
background runtime, restarts it with backoff if it exits (one notice per run
of failures), and stops it when it is turned off, removed, or Chaat quits. A
plugin whose `ready` offers no capability Chaat knows is stopped with a
notice and not restarted. Only one plugin gets rings: the first enabled
plugin in the list that is running and offers `call_alerts`.
The order is the list order in Settings › Plugins, which you can change with
**Reorder**. Settings › Calls has an **Open** link to that plugin's page.

Settings shows the plugin's name and description as it gives them in
`ready`, as plain shortened text (control and invisible formatting
characters removed), with an **About this plugin** link only for an HTTPS
address. Chaat's own line above the list always stays: plugins are separate
programs you install, not part of Chaat, so check what one does before you
use it. A plugin gets account IDs and sign-in names only after you approve
that in Settings › Plugins, and its file must match the hash Chaat stored
when you added it ([plugins.md](plugins.md#file-pinning)).

When the plugin asks to sign in, Chaat shows a **Sign in for call alerts**
card with the code, **Open sign-in page** and **Copy code**. Its wording is
Chaat's own: the only variable part is the sign-in name of the Chaat account
it is for, never the plugin's text. The plugin's name shows on a line of its
own, in quotes. Codes must be 4 to 32 letters, digits or dashes. Chaat can
show one kind of sign-in: a code entered on
one of four exact Microsoft device-login pages (`method: "device_code"`; the
list is in [plugins.md](plugins.md#accounts-forget-and-sign-in)). Other
methods and other pages are refused. Only an account's first code brings
the window forward.

### When a call rings

Chaat shows a ring card (and may raise its window, see below) at the top right: the caller,
whether it is a video or group call, and two actions:

| Action | What happens |
|---|---|
| **Place call** | Calls the caller back in Teams, the same way as **Call** in a chat, using the address Chaat knows from chat members. If Chaat does not know it, opens Teams on the web instead |
| **Open chat** | Opens the call's chat in Chaat: the group's thread when the ring carried one, otherwise the one-on-one chat with the caller, or starts that chat if Chaat knows the caller but has none yet. Unavailable for a caller Chaat has never seen |

The caller is matched by the Entra object ID the plugin sends
(`caller_object_id`), which is the `userId` of chat members Chaat already
has. Chaat makes no extra Graph request for it.

The card shows the caller's name from Chaat's own chat data when the
caller matches a chat member of that account. Otherwise it shows the name
the plugin sent, with a "Name from the plugin" line under it, because Chaat
cannot vouch for it. A thread from the ring is kept only if the matched
caller is a member of it; otherwise **Open chat** falls back to the
one-on-one chat with the caller, if any.

A ring raises the window at most once per call, and at most once every 10
seconds across all calls, so a plugin cannot keep taking focus. The card
still shows.

The card cannot answer the call. It closes when the plugin says the call
stops ringing (`ring_timeout_secs`, kept between 10 seconds and 2 minutes;
40 seconds when not given), on either action, or on dismiss. A repeat of
the same call replaces its card; at most three show.

### Several accounts

Chaat tells the plugin the ID (`oid.tid`) and sign-in name of each of its
accounts, never a token, and the plugin signs in to each separately.
Settings › Plugins lists every account under the plugin as **Listening for calls**
(its connection came up, and the plugin has reported no problem with it
since), **Waiting for sign-in** (with a **New code** button) or
**Connecting…**. Errors about an account name it.

A ring says which account it came in on. When Chaat has several accounts,
the card names it, and **Place call** and **Open chat** switch to that
account first, so the call uses its browser profile and the chat is its
own. A ring that names an account Chaat does not have is dropped.

Signing an account out of Chaat also clears the plugin's sign-in for it. With
call alerts on, the plugin hears at once (`forget`). Either way Chaat keeps
the account queued in its settings and lists it in `hello.forget` at every
start, until the plugin's `ready.forgotten` confirms the sign-in is gone, or
the account is signed in to Chaat again. A quit, crash or failed
credential store in between can't lose it. An account only removed from the list
stops listening but keeps its plugin sign-in.

### Protocol

The full protocol is in [plugins.md](plugins.md). In short: newline-delimited
JSON over the plugin's stdin and stdout, each message with `"v": 1` and a
`type`. Chaat drops lines with another version, lines over
64 KB, and types it does not know, so either side can be newer. A ring
field of the wrong type is read as absent rather than dropping the ring. Every
message below has an example in `tests/fixtures/plugin-protocol/`, kept
identical in the plugin's repository; tests on both sides check against it.

| Direction | `type` | Fields |
|---|---|---|
| Chaat → plugin | `hello` | `host: "chaat"`, `capabilities: ["call_alerts"]`, `accounts: [{id, username}]`, `forget` (optional: accounts signed out while the plugin was off) |
| Chaat → plugin | `accounts` | `accounts`: the new list. Removed accounts stop listening and keep their sign-in |
| Chaat → plugin | `forget` | `account`: clear the plugin's sign-in for it (sign-out) |
| Chaat → plugin | `sign_in` | `account`: offer a fresh code now, if it is waiting for one |
| Chaat → plugin | `shutdown` | |
| plugin → Chaat | `ready` | `capabilities` (must include `call_alerts`); `name`, `description`, `docs_url` (optional, shown in Settings); `forgotten`: the `hello.forget` accounts whose sign-ins are now gone. Sent only after clearing them |
| plugin → Chaat | `sign_in_required` | `method` (`device_code`; absent means the same), `verification_uri`, `user_code`, `account` and `username` (optional) |
| plugin → Chaat | `signed_in` | `account`, `username` |
| plugin → Chaat | `listening` | `account`, `username`; sent each time an account's connection comes up |
| plugin → Chaat | `incoming_call` | `id`, `caller`, `caller_object_id`, `video`, `group`, `account`, `username`, `ring_timeout_secs`, `thread_id` (optional). A call ringing two accounts shows a card for each |
| plugin → Chaat | `error` | `message`, shown as an error notice; with `account`, the notice names it and the account is no longer shown as listening |

`call_ended` is reserved for when a plugin can report it. Chaat passes a
plugin account IDs and sign-in names only when it declares the `accounts`
need and you have approved it, never tokens, settings or messages. It discards the plugin's stderr.
The fields beyond call alerts (`needs`, `options`, `presence`) are in
[plugins.md](plugins.md).

## API limits

Graph calling is for bots, with no delegated calling permission. ACS carries
additional usage charges; Chaat keeps media in Microsoft's own clients.
See the official sources in the [API reference](microsoft-api-research.md#calling).

Chaat has no embedded browser engine and uses no undocumented Teams endpoints
or call-control protocols. The optional call-alerts plugin does (push service);
it is a separate program, outside Chaat. Ringing, mute, camera and leave controls belong to
the Teams client carrying the call.

## Open questions

These still need a real browser, meeting and tenant:

| Check | Current fallback |
|---|---|
| Placement with an already-running browser and Windows display scaling | Chaat docks itself; browser placement is best effort |
| Teams' pre-join page and remembered browser choice | Follow Teams' own join prompts |
| Browser session reuse and creation of missing account profiles | Sign in to Teams on the web; separate profiles are optional |
| Presence timing when entering or leaving a real call | One-minute grace period and **Done** |
| Focusing the exact call window | **Show call** activates the browser on macOS |
| Call-alerts plugin with a real group call, and with several accounts signed in live | Tested with fixtures only; one live account and one-on-one rings were verified before multi-account support |
