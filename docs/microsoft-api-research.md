# Microsoft API reference

API constraints Chaat relies on. [Feature support](features.md) records what
is implemented and verified; [permissions](permissions.md) maps features to
scopes, and [app registration](azure-app-registration.md) covers setup.

Chaat uses documented Microsoft Graph APIs with delegated permissions and
Entra ID sign-in. It never impersonates Microsoft's clients or calls
undocumented Teams endpoints.

## Identity and consent

- Use a public client with authorization code + PKCE, a loopback redirect,
  and no client secret. Device code is a fallback and may be blocked by
  Conditional Access. See Microsoft's [code flow][code-flow] and
  [device code flow][device-flow].
- Chaat registers `http://localhost` and binds a free port. Entra ignores
  the port when matching localhost redirect URIs. See [redirect rules][redirect].
- Chaat's Teams features require work or school accounts. The
  [chat API][chats] does not support delegated personal Microsoft accounts.
- Permission flags alone do not guarantee user consent. None of Chaat's
  sign-in scopes needs admin consent by Microsoft's definition, but a
  tenant's [user-consent policy][consent] decides what users may approve,
  and many tenants block `Chat.ReadWrite` and `Calendars.Read`.
  [Publisher verification][publisher] also affects multi-tenant apps.
  Expect an administrator to approve Chaat in many tenants. Request optional
  scopes only when their feature is used.

## Graph behavior

| Area | Constraint and official reference |
|---|---|
| [Chat list][chats] | Maximum page size 50; expanded members are capped at 25 |
| [Chat messages][messages] | Maximum page size 50; descending creation/modification order, with matching date filters; follow paging links |
| Notes to yourself | Not in Microsoft's documentation. The chat id is `48:notes`; `GET /chats/48:notes` answers 400 and the chat is missing from `/me/chats`, but `GET`/`POST /chats/48:notes/messages` work with Chat.ReadWrite. It does not exist until the person first writes to themselves (404). Treat it as fragile: a failure must leave the rest of the chat list alone. A `/chats/getAllMessages` subscription may send no notification for a message to yourself, so Chaat updates this chat when the chat list refreshes |
| [Message export][export] | `GET /chats/getAllMessages` is application-only; it is not the delegated notification resource below |
| [Channel messages][channels] | Chaat uses `ChannelMessage.Read.All` for reads, requested incrementally with admin consent |
| [Presence change notifications][presence-push] | One presence subscription per person and app, for at most an hour; the bulk form `communications/presences?$filter=id in (…)` follows up to 650 people. Without resource data, each notification names the person, and Chaat fetches their presence. Graph cannot change whom a subscription follows, so Chaat replaces it when a new contact appears. By that rule a second device on the same account and app cannot hold its own presence subscription and falls back to fetching on look (not tested with two devices; Graph answers `409 ExtensionError` while an earlier run's presence subscription still exists). Bulk subscriptions are refused with `400 ExtensionError` when the list includes users Graph cannot follow, even ones in the same organisation whose presence `getPresencesByUserId` returns; the message names them: "Subscription failed for some users. Remove the failing users and retry. (id, id)" (not in Microsoft's documentation). Users who left the organisation can cause it. Chaat removes the named users, remembers them for a week, and retries; it searches for the longest accepted list only if a 400 names nobody |
| [Listing subscriptions][subscription-list] | With delegated permissions, `GET /subscriptions` returns only the subscriptions this app created for the signed-in person, needing no permission beyond the ones that created them (`Presence.Read.All` for presence); paged with `@odata.nextLink`. Chaat uses it only to clear its own presence subscription after a 409 |
| [Presence sessions][presence] | Sessions expire; Chaat updates its session only on input and clears it at sign-out or exit |
| [Calendar events][calendar] | Use `onlineMeeting.joinUrl` for meeting links; Chaat requests `Calendars.Read` |
| [Meeting lookup][meeting] | Lookup by `JoinWebUrl` can return the meeting chat; Chaat requests `OnlineMeetings.Read` on first use |
| Meeting ID lookup | `GET /me/onlineMeetings?$filter=joinMeetingIdSettings/joinMeetingId eq '…'` with OnlineMeetings.Read. Chaat uses it for the join box when someone types a meeting ID. It may not find every meeting, and older meetings may have no meeting ID, so Chaat asks for the link when nothing is found |
| Hide and unhide chats | `POST /chats/{id}/hideForUser` and `unhideForUser` with Chat.ReadWrite; the body names the user (`id`, `tenantId`). The chat's `viewpoint.isHidden` says whether it is hidden. No change notification, so Chaat sees changes made elsewhere on the next fetch |
| Mark chat as unread | `POST /chats/{id}/markChatUnreadForUser` with Chat.ReadWrite. No change notification |
| Pinned messages | `GET /chats/{id}/pinnedMessages?$expand=message` (Chat.Read or Chat.ReadWrite), `POST /chats/{id}/pinnedMessages` (ChatMessage.Send), `DELETE /chats/{id}/pinnedMessages/{id}` (Chat.ReadWrite). Chats only. No change notification; the system messages for a pin or unpin arrive with the chat's messages, and Chaat fetches the pins again when it sees one |
| Status message expiry | `setStatusMessage` takes an optional `expiryDateTime` (a `dateTime` and `timeZone`). Chaat sends the time in UTC. Presence.ReadWrite |
| Chat message links | `webUrl` is null for chat messages in Microsoft's samples, so Chaat builds a Teams deep link from the chat and message IDs. Channel messages carry a `webUrl`, which Chaat uses as is |
| [Automatic replies][auto-replies] | `GET /me/mailboxSettings/automaticRepliesSetting` and `PATCH /me/mailboxSettings` with `MailboxSettings.ReadWrite` (user-consentable). Messages are HTML; Chaat writes the schedule in UTC. Teams derives its Out of Office state from these replies; how soon Teams shows a change is not documented. Chaat treats 404 and `MailboxNotEnabledForRESTAPI` as "no Exchange Online mailbox" (error codes not tested against a real tenant) |
| [Message search][search] | Delegated search returns messages the person sent or received, with a subset of properties and `from`/`size` paging |
| [Resumable uploads][uploads] | Upload chunks to the returned `uploadUrl` without the Graph Authorization header |

For message attachments and sharing permissions, see [permissions](permissions.md)
and [privacy](privacy.md). Do not fetch message-provided images from other hosts
or send Graph credentials outside the Graph origin.

## Chat members

The people panel in group and meeting chats uses three calls:

- `GET /chats/{id}/members` lists the full member list. `POST /chats/{id}/members`
  adds people, one POST per person. Both use `Chat.ReadWrite`, which Chaat
  already requests at sign-in. Microsoft lists it as a higher-privileged
  permission for both calls. No new scope is needed.
- `DELETE /chats/{id}/members/{membership-id}` leaves a group chat. Only
  `ChatMember.ReadWrite` allows it, among delegated permissions. It needs admin
  consent, so Chaat requests it only when the person tries to leave a group
  chat and then clicks **Request access**.
- Adding history: "Don't include chat history" omits
  `visibleHistoryStartDateTime`. "Include all chat history" sends
  `0001-01-01T00:00:00Z`.

Microsoft documents roster changes for group chats (chatType group) and fixes
the roster of one-on-one chats; it does not document adding or removing members
of meeting chats, and notes that methods on a chat tied to an online meeting can
affect the meeting. Chaat offers Add people and Leave in meeting chats as Teams
does; a refusal from Graph shows as an error. The presence subscription and
the chat-list presence fetch cover members of group chats only, not meeting
chats. Opening any chat, a meeting chat included, fetches presence once for
up to 20 of its other members, and the panel shows dots for the members Chaat
has presence for.

The chat list's `$expand=members` returns at most 25 members. The panel
therefore fetches the full list with `GET /chats/{id}/members` when it opens.
That is a fetch the person causes, never a poll.

## Refresh and throttling

Microsoft's [Teams polling requirements][polling] allow repeated fetches
when a person visits or refreshes a view, but restrict unattended polling
for changes to once a day. Chaat adds no polling timers; every refresh of a
cached list goes through `sync::Freshness::claim`. Lookups for what the person
is looking at or typing (names, photos, presence of an open chat, inline
images, searches) are deduplicated instead, and searches wait for typing to
pause. Refresh messages by date range where supported.

[Chat notifications][notifications] support delegated subscriptions for
`/users/{id}/chats/getAllMessages` and individual chats. Chaat uses a
self-hosted HTTPS relay and requests no resource data, so the relay receives
no message content. Respect subscription expiration and lifecycle notices.
The implementation and deployment steps are in [realtime.md](realtime.md).

Honor `429` and `Retry-After`; do not retry immediately. Microsoft's
[throttling guidance][throttling] and [service limits][limits] cover limits
per app, tenant, resource and user. Check the source when changing request
patterns rather than relying on a copied limit table.

## Calling

Graph's [create-call API][calls] supports bots, not delegated user calling.
Microsoft's [Teams interop pricing][pricing] distinguishes calls in its own
Teams clients from billable ACS usage. Chaat opens documented
[Teams deep links][deep-links] and keeps media and call controls in Teams.
See [calling.md](calling.md) for current behavior and remaining live checks.

## Recordings

Chaat reads the link to a meeting recording from the chat message Teams
posts for a recording, the [`callRecordingEventMessageDetail`][rec-event]
system event (`callRecordingUrl`, `callRecordingDisplayName`,
`callRecordingDuration`, `callRecordingStatus`). It keeps the link only when
the status is `success` and the address is an https SharePoint link, and
opens it in the browser. This adds no scope: the event comes with the chat's
messages (`Chat.ReadWrite`). Learn describes the event both as generated
"when a call recording starts" and, in the [event list][event-list], as "Call
recording is available", so Chaat relies on the status. The link's exact host
has not yet been checked against a live payload.

Recordings live in the organizer's OneDrive (the recorder's for 1:1 and group
calls, the channel's SharePoint site for channel meetings), and invitees in
the organization get read access ([recording storage][rec-change]).

Chaat does not use the recording APIs:

- [`GET /me/onlineMeetings/{id}/recordings`][rec-list] needs
  `OnlineMeetingRecording.Read.All`, which needs admin consent.
- [Getting the recording content][rec-get] is limited to the organizer by
  default.
- They work only while the meeting has not expired: an online meeting
  expires 60 days after its start or end, extended when it is used again
  ([onlineMeeting][online-meeting]).
- [`getAllRecordings`][rec-all] works with application permissions only.

Scopes are listed in the [permissions reference][perm-ref].

## Terms and branding

Keep the [Microsoft APIs Terms of Use][api-terms] and
[Identity Platform Terms][identity-terms] as the source for registration,
minimum permissions/data, privacy, user terms, and limits on use. Provide
the privacy statement and revocation instructions in [privacy.md](privacy.md)
and [permissions.md](permissions.md).

Follow Microsoft's [trademark guidelines][trademarks]. Chaat is independent
and works with Microsoft Teams; names and artwork must not imply endorsement.

[code-flow]: https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow
[device-flow]: https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code
[redirect]: https://learn.microsoft.com/en-us/entra/identity-platform/reply-url
[chats]: https://learn.microsoft.com/en-us/graph/api/chat-list?view=graph-rest-1.0
[messages]: https://learn.microsoft.com/en-us/graph/api/chat-list-messages?view=graph-rest-1.0
[export]: https://learn.microsoft.com/en-us/graph/api/chats-getallmessages?view=graph-rest-1.0
[channels]: https://learn.microsoft.com/en-us/graph/api/channel-list-messages?view=graph-rest-1.0
[presence]: https://learn.microsoft.com/en-us/graph/api/presence-setpresence?view=graph-rest-1.0
[presence-push]: https://learn.microsoft.com/en-us/graph/changenotifications-for-presence
[subscription-list]: https://learn.microsoft.com/en-us/graph/api/subscription-list?view=graph-rest-1.0
[calendar]: https://learn.microsoft.com/en-us/graph/api/resources/event?view=graph-rest-1.0
[meeting]: https://learn.microsoft.com/en-us/graph/api/onlinemeeting-get?view=graph-rest-1.0
[auto-replies]: https://learn.microsoft.com/en-us/graph/api/user-update-mailboxsettings?view=graph-rest-1.0
[search]: https://learn.microsoft.com/en-us/graph/search-concept-chat-messages
[uploads]: https://learn.microsoft.com/en-us/graph/api/driveitem-createuploadsession?view=graph-rest-1.0
[notifications]: https://learn.microsoft.com/en-us/graph/teams-changenotifications-chatmessage
[polling]: https://learn.microsoft.com/en-us/graph/api/resources/teams-api-overview?view=graph-rest-1.0#polling-requirements
[throttling]: https://learn.microsoft.com/en-us/graph/throttling
[limits]: https://learn.microsoft.com/en-us/graph/throttling-limits
[consent]: https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies
[publisher]: https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview
[calls]: https://learn.microsoft.com/en-us/graph/api/application-post-calls?view=graph-rest-1.0
[pricing]: https://learn.microsoft.com/en-us/azure/communication-services/concepts/pricing/teams-interop-pricing
[deep-links]: https://learn.microsoft.com/en-us/microsoftteams/platform/concepts/build-and-test/deep-link-workflow
[rec-event]: https://learn.microsoft.com/en-us/graph/api/resources/callrecordingeventmessagedetail?view=graph-rest-1.0
[rec-list]: https://learn.microsoft.com/en-us/graph/api/onlinemeeting-list-recordings?view=graph-rest-1.0
[rec-get]: https://learn.microsoft.com/en-us/graph/api/callrecording-get?view=graph-rest-1.0
[rec-all]: https://learn.microsoft.com/en-us/graph/api/onlinemeeting-getallrecordings?view=graph-rest-1.0
[perm-ref]: https://learn.microsoft.com/en-us/graph/permissions-reference
[rec-change]: https://learn.microsoft.com/en-us/microsoftteams/tmr-meeting-recording-change
[online-meeting]: https://learn.microsoft.com/en-us/graph/api/resources/onlinemeeting?view=graph-rest-1.0
[event-list]: https://learn.microsoft.com/en-us/graph/api/resources/eventmessagedetail?view=graph-rest-1.0
[api-terms]: https://learn.microsoft.com/en-us/legal/microsoft-apis/terms-of-use
[identity-terms]: https://learn.microsoft.com/en-us/legal/microsoft-identity-platform/terms-of-use
[trademarks]: https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks
