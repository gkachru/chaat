# Privacy and security

- **No telemetry, no Chaat servers.** Chaat talks only to
  `login.microsoftonline.com` (sign-in) and `graph.microsoft.com` (your
  data). Additional connections: a file larger than 4 MiB is
  uploaded to the one-time upload URL Graph returns (your organisation's
  OneDrive host), without the Graph token; and an optional `chaat-relay` that
  you run yourself, or a push plugin you add that receives Graph's
  notifications through its own tunnel (for the maintainer's plugin,
  Cloudflare), in both cases seeing notification envelopes only
  ([realtime.md](realtime.md)); and GitHub for app update
  checks and downloads. Automatic app updates are checked at most daily and
  can be disabled in Settings. No Microsoft credentials or account data are
  sent to GitHub ([updates.md](updates.md)). Chaat opens a link in your
  browser only when you click it: Teams links, links in messages and cards,
  meeting recordings on SharePoint, a plugin's **About this plugin** link and
  sign-in pages.
- **Setup files** stay local. Chaat reads or writes a `.chaatsetup` file only
  where you choose, never uploads it, and never logs its path, contents or
  passphrase. The relay access key it may carry is stored in the OS credential
  store ([setup-file.md](setup-file.md)).
- **Plugins** are separate programs you add and turn on in Settings ›
  Plugins ([plugins.md](plugins.md)). A plugin never receives your tokens or
  your settings. It receives message text only if it offers `agents`, declares
  the `messages` need and you approve it, and then only the cached text of
  chats and channels you put on that account's allow list (nothing by
  default; no HTML, image URLs or attachment links), plus new messages and
  your own sends in them. An agent's read never triggers a Graph request, and
  its proposed replies and reactions are sent only when you click. Chaat
  never logs message text or what an agent read. Chaat sends it the ID and sign-in name of
  each of your accounts, your busiest presence (availability and activity
  across accounts, never the status message), or the random channel names of
  a push plugin (no account IDs), only when the plugin declares
  that need and you approve it in Settings › Plugins, plus the values of the
  plugin's own options. A push plugin sees subscription ids, `clientState` and
  resource paths, never tokens or message text. An agents plugin gets no
  tokens either. Secret option values are kept in the OS credential
  store, not in files, and are not logged. A plugin file that changed since
  you added it does not run until you allow it. These needs limit what Chaat
  sends, not what the program can reach: a plugin runs as you and can read
  what you can, including your files and, likely, the credential store that
  holds Chaat's tokens. What a plugin does with its data, and what network
  access it has, is its own business: read its documentation and add only
  plugins you trust.
- **Calls** run in Teams on the web, in your own Edge or Chrome (or the
  browser you choose). Chaat starts the browser with the meeting or call link
  and nothing else, and opens its own Microsoft sign-in page there so Teams
  finds the same session. It never reads the browser's files, cookies or
  pages, and never passes it a token. Join links are not logged.
- **Your password never reaches Chaat.** You sign in on Microsoft's page in
  your browser. Chaat receives a one-time code on `localhost` and exchanges it
  with a PKCE verifier only it knows.
- **Tokens.** The refresh token is stored in the OS credential store
  (Keychain, Windows Credential Manager, Secret Service), never in files.
  A Credential Manager entry holds at most 1280 characters, so on Windows a
  longer token is split across numbered entries (`refresh-token:<account>#<n>.<i>`)
  and the main entry only says where its parts are.
  The push relay's access key, if you set one, is stored there too
  (`relay-key`) and sent only to your relay.
  Access tokens live only in memory. Tokens are only ever sent to the Graph
  origin: paging links and image URLs on other hosts are refused.
- **Least privilege.** Delegated permissions only, requested incrementally;
  see [permissions.md](permissions.md).
- **Local data.** The cache of your chats, channels, meetings, photos and
  downloaded Graph-hosted images, plus your drafts and your outbox (the text
  of messages not sent yet), is a SQLite file in a folder only your user
  can read (0700/0600 on macOS and Linux).
  It is not encrypted yet; full-disk encryption (FileVault, BitLocker, LUKS)
  protects it at rest. Encrypting it with a key held in the credential store
  is on the roadmap. Signing out deletes it. Encoded message images use a
  128 MiB least-recently-used cache per account. The SQLite file can also
  contain unused pages and journal data.
- **Files on disk.** Chaat keeps these in its config, data and state folders:
  - `settings.json` (config): your preferences, each account's ID and its
    rail name, icon and color, the agents allow list (`agent_access`), each
    plugin's path, file hash, approved and refused needs and non-secret
    option values, muted chat ids, the ids and names of people you chose
    "Notify when available" for, the relay address and the window's size.
  - Per account, under `accounts/<account>/` (data): `chaat.db` (the cache
    above), `account.json` (the account's ID, display name, sign-in name,
    tenant, granted permissions and the client ID it signed in with),
    `push-subscriptions.json` (ids of Graph subscriptions to delete after a
    run that ended without deleting them) and `presence-refused.json` (ids
    of people whose presence Graph refused to follow, with when).
  - `chaat.log`, `chaat.log.1` (the previous run's log), `panic.log` and
    `window.ron` (saved window state) in the state folder.
  - Secrets are never in these files. They go to the OS credential store
    under `refresh-token:<account>`, `relay-key` and, for a plugin's secret
    options, `plugin-option:<plugin id>:<option id>`.
  - Message text is never written to the logs.
- **Notifications** ([notifications.md](notifications.md)). Notification text
  (titles, message previews, names) is never logged. Settings keep the ids of
  chats you muted and the ids and names of people you chose "Notify when
  available" for, in the settings file. On Windows, so that toasts show as
  Chaat with its icon, Chaat writes the per-user registry key
  `HKCU\Software\Classes\AppUserModelId\app.chaat.Chaat` (values
  `DisplayName` and `IconUri`) and the icon file `chaat-notification.png` in
  Chaat's data folder. To remove them, delete that key (for example with
  `reg delete HKCU\Software\Classes\AppUserModelId\app.chaat.Chaat /f`) and
  the file. Chaat writes them again the next time it starts.
- **Logs** never contain message text, tokens, authorization codes or URLs
  with query strings. Panics are logged without their payload
  (fastframe-log).
- **Files you send** are uploaded to your own OneDrive, in the
  `Microsoft Teams Chat Files` folder Teams uses too, and shared read-only
  with the people in that chat. In a channel, Chaat creates a view link for
  your organisation, since a channel's members aren't known in advance.
  Remove the file or its sharing in OneDrive to revoke access.
- **Pasted images** stay in memory as PNGs until you send the message. They
  use Graph hosted content, so the image follows the conversation's access
  rules and needs no OneDrive sharing link or Files.ReadWrite permission.
  Clipboard contents are read only when you paste into a focused composer.
- **Presence.** While "Show me as available while I use Chaat" is on (the
  default), Chaat tells Microsoft you are active when you use it, at most
  every 4 minutes and only after input. Microsoft shows you as inactive
  5 minutes after the last update and Away 5 minutes later. Chaat ends the
  session when you quit or sign out; otherwise it expires after an hour.
  Nothing is sent until you grant Presence.ReadWrite by setting your status
  once.
- **Automatic replies** (Out of Office) are read from your mailbox settings
  only when you open their settings page, and changed only when you save
  them there. Chaat keeps them in memory, not in its cache.
- **Call status** comes from your own presence: fetched when you return to
  Chaat and, with the relay, announced by a presence notification that
  carries no presence data.
- **Several accounts** each have their own cache folder and refresh token.
  Signing out of one removes only its data.
- **Images in messages** are only fetched from Graph's own hosted content.
  Images hosted elsewhere are not loaded, so senders can't track when you
  read a message.
- **Links in calendar invitations** are only offered as "Join" when they
  point to a Teams host.

Report security issues through [the private reporting process](../SECURITY.md),
rather than in a public issue.
