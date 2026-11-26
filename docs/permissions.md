# Permissions

Chaat uses **delegated** Microsoft Graph permissions only. It acts as the
signed-in person, and can never see more than that person can in Teams.
It asks for the smallest set that covers its features. Permissions that only
one feature needs are requested the first time someone uses that feature
(incremental consent): Chaat explains why, then opens Microsoft's consent page.
The two that need admin consent, `ChannelMessage.Read.All` and
`ChatMember.ReadWrite`, are requested when someone opens a channel or first
leaves a group chat.

Pasting images into a new message uses `hostedContents` in the ordinary
send request, covered by `ChatMessage.Send` or `ChannelMessage.Send`.
It does not request `Files.ReadWrite` or create a OneDrive sharing link.

Hide, unhide, mark as unread, pinned messages, status expiry and join by meeting ID add no scope: each call uses one of the permissions in the table below.

Call alerts need no Chaat permission. The optional call-alerts plugin is a
separate program with its own sign-in and consent (Teams' own client); Chaat
never shares its tokens with it ([calling.md](calling.md#call-alerts-plugin)).

Agents (an optional plugin that reads cached messages and proposes replies and
reactions, [plugins.md](plugins.md#agent-plugins)) need no new Graph
permission. Reads come from Chaat's cache and call nothing. A reply you
approve is an ordinary send (`Chat.ReadWrite` or `ChatMessage.Send` in chats,
`ChannelMessage.Send` in channels), and an accepted reaction is an ordinary
`setReaction`. Channel text reaches an agent only if Chaat already holds it,
which needs `ChannelMessage.Read.All`.

| Permission | Requested | Endpoints | Admin consent |
|---|---|---|---|
| `openid`, `profile`, `offline_access` | At sign-in | Token endpoint (identity, refresh token) | No |
| `User.Read` | At sign-in | `GET /me` | No |
| `User.ReadBasic.All` | At sign-in | `GET /users/{id}/photos/96x96/$value`, `GET /users?$search=…` (people search for new chats and channel mentions), `GET /users/{id}?$select=id,displayName` (names Graph leaves out of "added to the chat" events) | No |
| `Chat.ReadWrite` | At sign-in | `GET /me/chats`, `GET /chats/{id}/messages`, `PATCH …/messages/{id}`, `POST /users/{id}/chats/{id}/messages/{id}/softDelete`, `POST /chats/{id}/markChatReadForUser`, `POST /search/query`, `POST /subscriptions` on `/users/{id}/chats/getAllMessages` (push relay only), `GET /chats/{id}/members` and `POST /chats/{id}/members` (people panel), `POST /chats/{id}/hideForUser`, `…/unhideForUser`, `…/markChatUnreadForUser` (hide and mark unread), `DELETE /chats/{id}/pinnedMessages/{id}` (unpin), `GET /chats/{id}/pinnedMessages` (list pins) | Excluded from the default user-consent policy |
| `ChatMessage.Send` | At sign-in | `POST /chats/{id}/messages`, `POST …/replyWithQuote`, `setReaction`, `unsetReaction`, `POST /chats/{id}/pinnedMessages` (pin) | No |
| `Team.ReadBasic.All` | At sign-in | `GET /me/joinedTeams` | No |
| `Channel.ReadBasic.All` | At sign-in | `GET /teams/{id}/allChannels` | No |
| `ChannelMessage.Send` | At sign-in | `POST /teams/{id}/channels/{id}/messages`, `…/replies`, reactions | No |
| `Presence.Read.All` | At sign-in | `POST /communications/getPresencesByUserId`, `GET /me/presence`, `POST /subscriptions` on `communications/presences/{id}` or `communications/presences?$filter=id in (…)` (your own presence, your one-on-one contacts' and your group chats' members', push relay only), and `GET /subscriptions` to find Chaat's own presence subscription left by a crashed run when Graph reports a conflict | No |
| `Calendars.Read` | At sign-in | `GET /me/calendarView` | Excluded from the default user-consent policy |
| `Chat.Create` | First new chat | `POST /chats`, then `GET /chats/{id}?$expand=members` | No |
| `Files.ReadWrite` | First file sent | `PUT /me/drive/root:/Microsoft Teams Chat Files/{name}:/content`, `POST …:/createUploadSession` (above 4 MiB), `GET /me/drive/items/{id}`, `POST /me/drive/items/{id}/invite` (chats), `POST …/createLink` (channels) | No |
| `Presence.ReadWrite` | First status change, or the presence session | `POST /users/{id}/presence/setUserPreferredPresence`, `clearUserPreferredPresence`, `setStatusMessage` (with `expiryDateTime` for "Clear after"), `setPresence`, `clearPresence` | No |
| `OnlineMeetings.Read` | First "Meeting chat" during a call, or first join by meeting ID | `GET /me/onlineMeetings?$filter=JoinWebUrl eq '…'` (finds a meeting's chat from its join link), `GET /me/onlineMeetings?$filter=joinMeetingIdSettings/joinMeetingId eq '…'` (finds a meeting's join link from its meeting ID) | No |
| `MailboxSettings.ReadWrite` | First visit to Settings › Out of office, or "Out of office…" in the account menu | `GET /me/mailboxSettings/automaticRepliesSetting`, `PATCH /me/mailboxSettings` (automatic replies only) | No |
| `ChatMember.ReadWrite` | First group chat left | `DELETE /chats/{id}/members/{membership-id}`. No other delegated permission allows it | **Yes** |
| `ChannelMessage.Read.All` | First channel opened | `GET /teams/{id}/channels/{id}/messages`, `…/replies` | **Yes** |

Meeting recordings need no scope of their own: Chaat reads the link from the
recording event in a meeting chat's messages (`Chat.ReadWrite`), and opens it
in the browser.

## Not requested

| Permission | Would enable | Why not yet |
|---|---|---|
| `ChannelMessage.ReadWrite` | Editing and deleting channel posts | Requires admin consent; the interface hides these actions without it |
| `OnlineMeetings.ReadWrite`, `Calendars.ReadWrite` | Creating meetings | Not in the interface yet |
| `Calls.*` | Graph calling | Application-only (bots). See [calling.md](calling.md) |

## Revoking access

Sign out in Chaat (**Settings → Account → Sign out**) to delete the refresh token from
the OS credential store and remove the account's local cache. To revoke the
grant at Microsoft, open [My Apps](https://myapps.microsoft.com) or ask an
administrator to remove the app's consent in Entra ID.
