# Feature support

Status as of Chaat 0.1 (unreleased).

- **Works**: implemented against Microsoft Graph, unit-tested against a mock
  Graph server and drawn in the headless UI tests. Automated coverage and
  maintainer-confirmed live-account acceptance are listed below.
- **Hand-off**: opens Microsoft Teams through documented deep links.
- **Experimental / planned**: designed, not built.
- **Not possible**: no official API allows it.

## Messaging

| Feature | Status | Notes |
|---|---|---|
| Chat list (1:1, group, meeting chats), unread state, filters for unread, group and meeting chats | Works | `GET /me/chats` with members and newest message; filters use `chatType` locally |
| Notes to yourself (the chat with yourself) | Works, relies on an undocumented chat id | Teams keeps it as chat `48:notes`, which `GET /me/chats` leaves out and `GET /chats/48:notes` refuses (400). Its messages list and send through the documented chat-message endpoints, so Chaat builds the list entry from the newest message and sorts it with the other chats by activity. Accounts that never wrote to themselves get 404 and no entry. Verified on one live account. Push sent no notification for it in one live test (18 Nov 2026), so it updates when the chat list refreshes, not instantly |
| Message history with paging | Works | 50 per page; follows Graph paging links, with a timestamp cursor after reopening; cached older pages load first and preserve the visible row |
| Send messages (rich text from light markdown) | Works | Optimistic, with retry on failure. Enter sends; Shift+Enter adds a line, including while editing or showing mention suggestions. Long drafts scroll inside a bounded editor, keeping history and composer controls visible |
| Quote replies in chats | Works | `replyWithQuote` |
| Reactions (any emoji) | Works | `setReaction` / `unsetReaction`; legacy names mapped to emoji |
| Edit and delete your messages | Works | Chats: Chat.ReadWrite. Channels: needs ChannelMessage.ReadWrite (admin), hidden otherwise |
| Mark read | Works | `markChatReadForUser` when a chat is opened. A "Last read" line marks where unread messages begin, from your own read time in `viewpoint`. Messages that arrive in the open chat while Chaat is behind another window stay unread until you return to it |
| Rich text: bold, italic, underline, strike, code, quotes, lists, links, mentions, emoji | Works | Native rendering of Teams HTML |
| Inline images (hosted content) | Works | Lazy, Graph-hosted only; other hosts are never fetched (privacy). Downloads have a bounded local cache. Thumbnails use reduced-resolution textures; originals are released after closing the viewer. Click an image to open the same fit/100% viewer as draft images, using the original decoded dimensions |
| File attachments: view | Works | Shown as file chips that open in the browser |
| File attachments: send | Works | Attach button or drag and drop, up to 1 GiB per file. Uploaded to your OneDrive folder `Microsoft Teams Chat Files` (resumable upload above 4 MiB), shared read-only with the chat's members (an organisation-wide view link in channels), and sent as a `reference` attachment. Files.ReadWrite, requested on first use |
| Paste images into the composer | Works | Fixed-size thumbnails crop without stretching. Click to preview the complete image, fit it to the window, or view at 100% with scrolling. Inline PNG hosted-content sends, just under 3 MiB combined per message; no Files.ReadWrite needed |
| @mentions in the composer | Works | Type `@`: suggestions from the chat's members, or a directory search in channels. Tab or Enter picks a suggestion and keeps focus in the composer; Space then inserts a space. Sent as `<at>` tags with a `mentions` array, so people are notified as in Teams |
| Adaptive Cards | Not yet | Shown as a placeholder with a hint to open in Teams |
| Local message search | Works | SQLite FTS5 over everything cached, instant |
| Microsoft Search (server-side) | Works | Merged into ⌘K results for queries of 3+ characters |
| Start a new chat | Works | ⌘N or the compose button: search your organisation's directory (User.ReadBasic.All), pick one or more people, optionally name a group. `POST /chats` with Chat.Create, requested on first use. An existing 1:1 chat opens instead of a new one |
| People panel in group and meeting chats: see members, add people (with or without chat history), leave | Works, leaving needs admin consent | People-count button in the header. Members with presence. Add people one at a time with `POST /chats/{id}/members` (Chat.ReadWrite, already granted), either without chat history or with all of it. Leave uses `DELETE /chats/{id}/members/{membership-id}` with ChatMember.ReadWrite, requested on first use and admin-approved in most tenants. Until it is approved, Chaat explains and offers "Request access" and "Open in Teams". The full member list is fetched when the panel opens, since chat lists show at most 25 members |
| Real-time updates | Works by default on intent; push works with a relay | Fetch on open, focus and refresh (Microsoft forbids background polling). With a self-hosted `chaat-relay`, Graph change notifications push new messages and presence changes. See [realtime.md](realtime.md). When Microsoft can't be reached, a dim offline marker in the sidebar header replaces error pop-ups until a fetch gets through; reads are retried once after a reset connection |
| New-message notifications | Partial | Whenever a fetch finds new messages in other chats. With the relay, also while Chaat runs in the background |

## Teams and channels

| Feature | Status | Notes |
|---|---|---|
| Joined teams and their channels (incl. private and shared) | Works | Team.ReadBasic.All, Channel.ReadBasic.All |
| Channel posts with threaded replies | Works, needs admin consent | ChannelMessage.Read.All. Without it, Chaat explains and offers "Request access" and "Open in Teams" |
| New posts and thread replies | Works | ChannelMessage.Send |

## Calls and meetings

| Feature | Status | Notes |
|---|---|---|
| Upcoming meetings (14 days) and Join | Works | Calendars.Read. Join opens the meeting in Teams on the web, beside Chaat (below) |
| Meeting reminders | Works | 5 minutes before, from the cached calendar (no network) |
| Start audio or video call | Works | Microsoft's call deep link in Teams on the web; Teams asks to confirm |
| Missed calls and call length in chats | Works, inferred | From the chat's `callEndedEventMessageDetail` system message: "Call ended after 4m 12s" from `callDuration`. "Missed call", with **Call back** (the call deep link to the chat's other members), when someone else placed a call (not a meeting) and nobody else joined. Graph has no answered or declined state, so a declined call also shows as missed, and a group call one other person answered does not. Shown after the chat is fetched, not while the call rings |
| Join by pasted link | Works | Teams hosts only |
| Where calls open | Works | Default: a chrome-less Edge or Chrome window (`--app`); without either, the default browser. The Teams app only if chosen in Settings. The browser is started with an argument list, never a shell, and only with Teams links ([calling.md](calling.md)) |
| Docking beside the call | Works | Chaat becomes a 380-point panel on the right of its frame and asks the browser to open the call on the left; restored when the call ends. Not on Wayland (no window positions) or for windows under 960 points |
| Call bar and call status | Works | Follows your own presence: on focus, and with the relay through a presence subscription (Presence.Read.All). Ends a minute after presence leaves the call. Shows calls started elsewhere too |
| Meeting chat beside the call | Works | From the cache by join link; otherwise `GET /me/onlineMeetings?$filter=JoinWebUrl eq '…'` with OnlineMeetings.Read, requested on first use |
| Signed in like Chaat | Works | Chaat's sign-in opens in the call browser, so Teams on the web finds the same Microsoft session; "Sign in to Teams on the web" in Settings for "Stay signed in"; optional browser profile per account and browser sign-out |
| Join on a phone | Works | QR code of the join link |
| Incoming-call alerts | Through an optional plugin | Graph cannot signal a ringing call. A separately installed, opt-in plugin supplies rings (`call-alerts` uses Teams' own push service and sign-in; see its README) and Chaat shows a ring card with **Place call** and **Open chat**, for each of Chaat's accounts (the plugin is told their IDs and sign-in names, never tokens); it cannot answer, and the card closes when the plugin says the call stops ringing. Ring detection was verified live on Windows with one tenant and an earlier build of the pipeline. The current plugin host and protocol are covered by tests only: protocol fixtures shared with the plugin, plugin-host tests against real fake-plugin processes, and headless UI tests of the ring card, sign-in card and Settings, on Windows and Linux. See [calling.md](calling.md#call-alerts-plugin) |
| Answering incoming calls, audio and video inside Chaat's window, call controls | Not possible at no cost | Graph calling is application-only (bots). Microsoft's licence covers call media only in its own clients; Azure Communication Services bills per participant-minute. See [calling.md](calling.md) |

## People and app

| Feature | Status | Notes |
|---|---|---|
| Presence of people in chats | Works | Presence.Read.All. Dots on one-on-one chats in the list, on message authors and in member lists. Fetched when you look: when a chat opens, and for one-on-one contacts and group-chat members when the chat list loads (start, foreground, refresh), at most once a minute per person. With the relay, one presence subscription follows you and up to 649 others, one-on-one contacts first, then group-chat members, most recent chats first, so dots change as people's status does; a burst of changes is one fetch after 1.5 seconds. People from other organisations, meeting-chat attendees, and members of large groups beyond the 25 that Graph lists per chat, update on look only |
| Setting your own availability and status message | Works | From the account menu (your picture in the rail): Available, Busy, Do not disturb, Be right back, Appear away, Appear offline, or Reset. `setUserPreferredPresence` with Graph's default expiry, `setStatusMessage`. Presence.ReadWrite, requested on first use. Clearing a message sends empty content, which Microsoft does not document (unverified) |
| Showing you as available while you use Chaat | Works | A Graph presence session (`setPresence`), refreshed at most every 4 minutes and only on input, so stepping away shows you inactive after 5 minutes and Away after 10. Ended at sign-out and exit, expires after an hour otherwise. Starts once Presence.ReadWrite is granted; can be turned off in Settings |
| Out of office (automatic replies) | Works | Settings › Out of office, also from "Out of office…" in the account menu. Off, on, or on during a time range (local time), a reply for your organization, and an optional reply for your contacts or everyone outside it. Teams shows you as out of office while replies are on; Outlook sends the same replies by email. Read when the page opens, saved with `PATCH /me/mailboxSettings`, then read back. MailboxSettings.ReadWrite, requested on first use. Needs an Exchange Online mailbox; accounts without one see a notice. Replies are edited as plain text, so formatting set in Outlook is lost on save |
| Profile photos | Works | Lazy, cached 7 days |
| Multiple accounts | Works | Add from the account menu or Settings; ⌘⇧A switches. Each account has its own backend, cache, credentials and push connection. Unread chats in other accounts show as a dot on your picture; their notifications still arrive. Signing in to an account twice keeps one |
| Light, dark, system theme, 8 shared palettes | Works | fastframe-theme presets |
| Integrated title bar | Works on Windows; compiled only on Linux | Minimize, maximize and close drawn in Chaat's header, with drag, double-click to maximize and edge resizing (fastframe-titlebar). On by default; "Use Chaat's own title bar" in Settings switches back to the system frame. macOS keeps its native traffic lights |
| Keyboard shortcuts, ⌘K switcher | Works | ⌘/ lists them |
| Tray / menu-bar item, keep running | Works | Off by default; with a self-hosted relay, push notifications continue while the window is closed |
| Desktop notifications | Works | notify-rust |
| Single running instance | Works | fastframe-instance |
| Performance overlay | Works | ⌘⇧P |
| Personal Microsoft accounts / Teams (free) | Not possible | Graph's Teams APIs support work and school accounts only |

## App updates

Official portable downloads and macOS bundles check GitHub daily and download signed updates in the background. Restart is explicit, with rollback if the new app fails to start. Package-manager and unmarked source builds use their existing update method. The first public release still needs to be published. See [updates.md](updates.md).

## Verification

| Coverage | Verification |
|---|---|
| Authentication, Graph, cache and calling logic | Unit tests and local mock servers: PKCE, token rotation, paging, retries, token isolation, writes, uploads, search, browser arguments, docking and call state |
| Backend and feature flows | `tests/backend_flow.rs` and `tests/features_flow.rs`: sign-in, session restoration, sign-out, messaging, files, new chats, mentions, presence and meeting-chat lookup |
| Push | `tests/push_flow.rs`: real relay and backend with simulated Graph, subscription validation, notifications and cleanup |
| Interface | `tests/ui_smoke.rs`: headless frames for every screen, dialogs, account switching and a 5,000-message chat |
| Native macOS | Apple Silicon, macOS 27; demo screenshots at 1280 × 800, call bar, docked panel and QR code; signed-in performance checks in [performance.md](performance.md) |
| Persistent macOS signing | Synthetic-certificate regression and maintainer-confirmed reopening without another Keychain prompt; setup in [macos-keychain.md](macos-keychain.md) |

**Live-account acceptance**, confirmed by the maintainer on 17 October 2026:
a fresh sign-in and consent flow, chat and channel reads and writes, file
sharing, presence, calendar, account switching, sign-out cleanup, and real
push notifications. This was a manual check with a real account.

Remaining checks and known issues:

- Native Windows and Linux windows have not been inspected. Both compile
  and pass tests in CI.
- A real call in the Teams web window has not been confirmed. See the
  [calling open questions](calling.md#open-questions).
- The Calls capability table clips at 760 × 700, observed in both themes
  during the macOS demo checks.
