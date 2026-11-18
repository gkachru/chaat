# Calling and meetings

Chaat opens calls in Microsoft Teams on the web and stays beside them for
chat and call status. Audio, video, screen sharing and call controls stay in
Teams. Chaat does not show incoming-call alerts or request microphone, camera
or screen-recording access.

The implementation has mock, headless UI and macOS screenshot coverage.
A real call has not yet been confirmed; see [open questions](#open-questions).

## Opening a call

Select **Call** in a chat, **Join** on a meeting, or paste a Teams meeting
link in the Calls page. By default, Chaat opens an Edge or Chrome app window
without browser tabs. If neither browser is installed, it uses the default
browser. The Teams desktop app is an explicit choice in Settings.

Settings → Calls and meetings controls:

| Setting | Behavior |
|---|---|
| Open in | A Teams web app window, the default browser, or the Teams app |
| Browser | An installed Edge or Chrome; Edge is preferred by default |
| Open all links in Edge or Chrome | Links in messages, the calendar and "Open in Teams" open in the chosen browser and the same profile as calls, instead of the default browser; off by default. `mailto:` links still use the default mail app. Shown whenever Edge or Chrome is installed |
| Move Chaat beside the call window during calls | Dock Chaat in a 380-point panel and restore its previous geometry afterward; on by default |
| A separate browser profile for each account | Optional, shown with multiple accounts; off by default |
| Signing out of Chaat also signs the browser out of Microsoft | Optional; off by default |
| Sign in to Teams on the web | Open Teams once to complete browser sign-in and choose whether to stay signed in |

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

## API limits

Graph calling is for bots, with no delegated calling permission. ACS carries
additional usage charges; Chaat keeps media in Microsoft's own clients.
See the official sources in the [API reference](microsoft-api-research.md#calling).

Chaat has no embedded browser engine and uses no undocumented Teams endpoints
or call-control protocols. Ringing, mute, camera and leave controls belong to
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
