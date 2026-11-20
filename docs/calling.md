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
| Call alerts | The call-alerts plugin's executable (chosen with the system file picker, or removed), **Show incoming calls**, and whether it is running. Off by default; see [below](#call-alerts-plugin) |

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
notifications arrive only after the call, and Graph calling is for bots.
Teams rings its own clients through a push service
that only accepts Teams' first-party sign-in. Chaat itself never uses either
(see [AGENTS.md](../AGENTS.md)). The separate, opt-in `call-alerts`
plugin does, so people who accept that trade-off can see who is calling.

Using it means signing in with Teams' own client ID and an undocumented
service, outside Microsoft's documented APIs and possibly against your
organization's policy. It is the person's choice; Chaat does not ship,
download or update it.

### Turning it on

Settings › Calls › Call alerts: **Choose plugin…** picks the executable and
turns **Show incoming calls** on. Chaat starts it as `<plugin> serve` on its
own background runtime, restarts it with backoff if it exits, and stops it
when the switch is turned off, the plugin is removed, or Chaat quits.

On first use (or when its stored sign-in expires) the plugin asks to sign in.
Chaat shows a **Sign in for call alerts** panel with the code, **Open
sign-in page** and **Copy code**. The plugin keeps its refresh token in the
system's credential store and tells Chaat when sign-in finished. Chaat
shows the panel only for HTTPS pages on Microsoft's sign-in hosts, and
brings its window forward for the first code only. If the plugin stops,
Chaat restarts it with backoff and shows one notice per run of failures.

### When a call rings

Chaat raises its window and shows a ring card at the top right: the caller,
whether it is a video or group call, and two actions:

| Action | What happens |
|---|---|
| **Place call** | Calls the caller back in Teams, the same way as **Call** in a chat, using the address Chaat knows from chat members. If Chaat does not know it, opens Teams on the web instead |
| **Open chat** | Opens the call's chat in Chaat: the group's thread when the ring carried one, otherwise the one-on-one chat with the caller, or starts that chat if Chaat knows the caller but has none yet. Unavailable for a caller Chaat has never seen |

The caller is matched by the Entra object ID in the ring (`8:orgid:<id>`),
which is the `userId` of chat members Chaat already has. Chaat makes no
extra Graph request for it.

The card cannot answer the call. Answering needs a Teams client that was
registered before the call started ringing; Teams on the web opened during
the ring does not receive it (tested). The plugin also cannot see when a
call ends, so the card closes after 40 seconds, on either action, or on
dismiss. A repeat of the same call replaces its card; at most three show.

### Several accounts

Chaat tells the plugin the ID (`oid.tid`) and sign-in name of each of its
accounts, never a token. The plugin listens for each one on its own
connection, in one process, and asks to sign in to each separately: the
panel names the account to sign in as, and a sign-in to a different account
is turned down with a new code. Settings lists every account as **Listening
for calls** (its connection came up, and the plugin has reported no problem
with it since), **Waiting for sign-in** (with **New code**) or
**Connecting…**. Errors reach Chaat once per run of failures, naming the
account.

A ring says which account it came in on. When Chaat has several accounts,
the card names it, and **Place call** and **Open chat** switch to that
account first, so the call uses its browser profile and the chat is its
own. A ring on an account the plugin has but Chaat does not still shows,
named by its sign-in name; **Place call** opens Teams on the web and **Open
chat** is unavailable. Signing an account out of Chaat also clears the
plugin's sign-in for it, even with call alerts off (Chaat then runs
`<plugin> logout <account>` once); an account only removed from the list stops
listening but keeps its plugin sign-in.

### Protocol

Newline-delimited JSON over the plugin's stdin and stdout, each message with
`"v": 1` and a `type`. Chaat ignores lines it does not understand, so either
side can be newer.

| Direction | `type` | Fields |
|---|---|---|
| Chaat → plugin | `hello` | `host: "chaat"`, `capabilities: ["call_alerts"]`, `accounts: [{id, username}]` (without `accounts`, the plugin serves every account it has stored) |
| Chaat → plugin | `accounts` | `accounts`: the new list. Removed accounts stop listening and keep their sign-in |
| Chaat → plugin | `forget` | `account`: clear the plugin's sign-in for it (sign-out) |
| Chaat → plugin | `sign_in` | `account`: offer a fresh code now, if it is waiting for one |
| Chaat → plugin | `shutdown` | |
| plugin → Chaat | `ready` | `capabilities` |
| plugin → Chaat | `sign_in_required` | `verification_uri`, `user_code`, `account` and `username` (optional) |
| plugin → Chaat | `signed_in` | `account`, `username` |
| plugin → Chaat | `listening` | `account`, `username`; sent each time an account's connection comes up |
| plugin → Chaat | `incoming_call` | `id`, `caller`, `caller_mri`, `video`, `group`, `account`, `username`, `thread_id` (optional, usually absent for one-on-one calls). A call ringing two accounts shows a card for each |
| plugin → Chaat | `error` | `message`, shown as an error notice; with `account`, the notice names it and the account is no longer shown as listening |

`call_ended` is reserved for when a plugin can report it. Chaat passes the
plugin its accounts' IDs and sign-in names only: no tokens, settings or
messages. It discards the plugin's stderr.

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
