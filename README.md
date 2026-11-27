<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="packaging/icons/chaat-wordmark-light.svg">
    <img src="packaging/icons/chaat-wordmark.svg" width="160" alt="Chaat">
  </picture>
</p>
<p align="center">A fast, native app that works with Microsoft Teams chats, channels and meetings.<br>
Rust and egui. No browser engine. Works with Microsoft Teams through Microsoft's official APIs.</p>

> **Status: early preview.** See
> [what works and what was verified](docs/features.md) for the tested scope
> and remaining platform checks.

## Why

Microsoft's Teams client is a large web application. Chaat is a purpose-built
native messaging client in the spirit of
[Spotifast](https://github.com/crmne/spotifast) and
[ZapFast](https://github.com/crmne/zapfast): one small binary, a window that
draws only when something changes, bounded image caches, and every view served
from a local cache first.

## Every account in one window

Sign in with as many work or school accounts as you use, across different
organizations. Each account is a tile in the rail on the left, named from its
sign-in domain, with an icon and color you can change. The Chaat logo opens
the **Unified** view: chats, teams, calendar and calls from every account in
one list, each row marked with its account.

- A group chat that two of your accounts share shows once. **Sending as**
  picks the account you reply from.
- The same meeting on two calendars shows once.
- Search (`⌘K`) covers every account.
- Availability set in Unified applies to every account that allows status
  changes. Status messages and Out of office stay per account.
- **Unified only** (Settings › Appearance) hides the tiles, and `⌘⇧A` cycles
  through Unified and each account.

Each account keeps its own sign-in, cache and push connection. Unified reads
what Chaat already has cached, so it needs no extra permissions and makes no
extra requests to Microsoft.

## Plugins

Chaat can run external programs that you install yourself and turn on in
Settings › Plugins. A plugin talks to Chaat over a small, versioned protocol
of JSON lines on standard input and output, so it can be written in any
language ([protocol](docs/plugins.md)).

| Capability | What a plugin can do |
|---|---|
| Push updates | Receive Graph's change notifications through a `cloudflared` tunnel, for real-time updates without hosting a relay ([how](docs/realtime.md#through-a-push-plugin)) |
| AI agents | Let an AI agent read the chats and channels you allow and propose replies and reactions. Nothing is sent until you click ([how](docs/plugins.md#agent-plugins)) |
| Presence display | Show your availability on a status light or another device |
| Call alerts | Tell Chaat that a call is ringing, which Graph cannot do. Chaat shows who is calling, with **Place call** and **Open chat** ([how](docs/calling.md#call-alerts-plugin)) |

Chaat stays in control of what a plugin gets:

- A plugin receives nothing until it asks for a specific kind of data and you
  approve it. It never receives sign-in tokens.
- Message text goes only to an agent plugin you approved, and only for the
  chats and channels on that account's allow list, which is empty by default.
- Chaat records the SHA-256 hash of each plugin file and won't run a changed
  file until you approve it again.
- Everything a plugin sends is treated as untrusted. Nothing happens until
  you click.
- Chaat never ships, downloads or updates a plugin. Plugins are published
  separately, and none is public yet.

## Features

| | Status |
|---|---|
| Chats: list, unread, filter by unread, group or meeting chats, history, send, quote replies, reactions, edit, delete, mark read | ✅ Implemented (Graph) |
| New chats, @mentions in the composer, sending files (OneDrive upload and sharing) | ✅ Implemented |
| People in group and meeting chats: members (with presence in group chats), add people (with or without chat history), leave | ✅ Implemented; leaving needs admin consent |
| Rich text, mentions, emoji, links, code, inline images | ✅ Implemented (native rendering of Teams HTML) |
| Images in chats and drafts | ✅ Cropped draft thumbnails; click either for the same full preview with fit and 100% views |
| Teams and channels, threaded posts and replies | ✅ Implemented; reading channel posts needs admin consent |
| Local full-text search plus Microsoft Search, ⌘K switcher | ✅ Implemented |
| Calendar, meeting reminders, Join | ✅ Implemented |
| Calls and meetings | ✅ In Teams on the web, in its own Edge or Chrome window beside Chaat, with the meeting chat and a call bar in Chaat; free, since the call runs in Microsoft's client ([how](docs/calling.md)) |
| Presence, photos | ✅ Implemented |
| Light, dark and system themes, 8 palettes, keyboard shortcuts, tray | ✅ Implemented |
| Notifications: mentions-only, Do not disturb, pause, mute, urgent repeats, reminders with Join and Snooze | ✅ Implemented; verified on Windows ([rules](docs/notifications.md)) |
| App updates | ✅ Daily checks and verified downloads from GitHub releases; restart when ready ([details](docs/updates.md)) |
| Real-time updates | ✅ On open, focus and refresh. ✅ Push through a self-hostable `chaat-relay`, or through a push relay plugin with a `cloudflared` tunnel instead of a hosted relay, since Microsoft forbids polling ([why](docs/realtime.md)) |
| Multiple accounts with a Unified view, setting your availability and status message | ✅ Implemented ([above](#every-account-in-one-window)) |
| Out of office (automatic replies) | ✅ In Settings and an account's menu, for accounts with an Exchange Online mailbox |
| Plugins: push, AI agents, presence display, call alerts | ✅ Plugin host and protocol implemented. ⚠️ Each capability needs a separately installed plugin, and none is public yet ([above](#plugins)) |
| Audio and video inside Chaat's own window, answering calls | ❌ No free official way: Microsoft's licence covers call media only in its own clients ([why](docs/calling.md)) |

The full matrix, including what was verified and how, is in
[docs/features.md](docs/features.md). API constraints and official references
are in
[docs/microsoft-api-research.md](docs/microsoft-api-research.md).

## Platforms

| Platform | Status |
|---|---|
| Windows (x86_64) | Main development platform, run with real accounts |
| Linux (x86_64) | Builds and passes the test suite (checked in WSL). A `.deb` for Ubuntu 22.04+ and Debian 12+. Not yet tried on a Linux desktop |
| macOS (Apple Silicon) | Checked on macOS 27 before Chaat started from Wren. Recent changes have not been built or run on a Mac |

## Getting started

### Build

```sh
git clone https://github.com/gkachru/chaat && cd chaat
cargo run --release
```

Linux needs `libxkbcommon-dev libwayland-dev libgl1-mesa-dev` (Debian and Ubuntu
package names) to build.

### Use it with your Microsoft 365 account

Chaat signs in through an app registration in your organisation's Entra ID.
It ships no shared client ID: Microsoft throttles per client ID across all
tenants, and most tenants block unverified multi-tenant apps.

1. Register the app (about five minutes, no secrets):
   [docs/azure-app-registration.md](docs/azure-app-registration.md).
2. Start Chaat, enter the **Application (client) ID** and your **tenant**, and
   select **Sign in with Microsoft**. Or set `CHAAT_CLIENT_ID` and
   `CHAAT_TENANT` in the environment.
3. In most tenants an administrator must approve `Chat.ReadWrite` and
   `Calendars.Read`, which the default consent policy excludes, and
   `ChannelMessage.Read.All` for channel posts, and `ChatMember.ReadWrite` for
   leaving group chats. Chaat shows an admin-consent link under
   **Settings → Advanced**. Starting a chat, sending a file, setting your status
   and leaving a group chat each ask for one more permission the first time. Each
   permission is explained in [docs/permissions.md](docs/permissions.md).

To skip typing the IDs and the relay access key, an administrator can create
an encrypted setup file in **Settings → Advanced** and send it to colleagues.
They drop it on Chaat's first screen and enter the passphrase
([docs/setup-file.md](docs/setup-file.md)).

Personal Microsoft accounts and Teams (free) are not supported; Graph's Teams
APIs serve work and school accounts only.

### Build packages

| Platform | Command |
|---|---|
| macOS | Set `CODESIGN_IDENTITY`, then `cargo build --release && packaging/macos/bundle.sh target/release/chaat Chaat.app 0.4.0` |
| Linux | `cargo build --release && packaging/linux/install.sh` (installs to `~/.local`) |
| Debian and Ubuntu (.deb) | `packaging/linux/deb.sh` builds `dist/chaat_<version>-1_amd64.deb` for Ubuntu 22.04+ and Debian 12+ (glibc 2.35). Needs `cargo-zigbuild`, `zig` and `cargo-deb`. |
| Windows | `cargo build --release` (icon and version embedded by `build.rs`) |

CI builds all three on every push to `main` and keeps them as workflow
artifacts. Version tags create a signed draft release with packages that
support automatic updates. Nothing is published as a release yet. See
[app updates and release steps](docs/updates.md).

On Debian and Ubuntu (amd64, Ubuntu 22.04+ or Debian 12+), install the `.deb`
from the releases page with `sudo apt install ./chaat_<version>-1_amd64.deb`.
Updates arrive as new `.deb` files on the releases page; the package does not
include the portable marker, so Chaat's self-updater stays off for it.

macOS bundles for signed-in use need a persistent code-signing identity so
Keychain approval survives updates. CI artifacts use ad-hoc signing for
development and can ask for approval again after each replacement. See
[macOS signing and recurring Keychain prompts](docs/macos-keychain.md).

## Keyboard

`⌘K` jump anywhere or search messages · `⌘N` new chat · `⌘1–5` sections ·
`Alt ↑/↓` previous or next conversation · `⌘⇧A` cycle Unified and accounts · `Enter` send ·
`Shift Enter` new line · `@` mention · `↑` edit last message · `⌘R` refresh ·
`⌘,` settings · `⌘/` all shortcuts. (`⌘` is Ctrl on
Windows and Linux.)

## Architecture

The views read state and emit actions, a single reducer applies them, and a
tokio runtime on its own thread does all network and disk work. Details are in
[docs/architecture.md](docs/architecture.md).

Built on [fastframe](https://github.com/crmne/fastframe) (fonts, colour emoji,
icons, palettes, tray, window shell, title bar, single instance, scrolling,
logging) and the egui and winit forks shared with Spotifast and ZapFast.

## Known Microsoft API limitations

- **Calling.** Graph's calling API is application-only and meant for bots;
  nothing lets a third-party desktop app call as the signed-in person for
  free. Azure Communication Services can, but bills every participant-minute
  to an Azure resource, since the Teams licence covers calls only in
  Microsoft's own clients. So Chaat opens calls in Teams on the web beside
  itself and follows them from there ([details](docs/calling.md)).
- **Real-time.** Polling Graph for Teams changes more than once a day breaks
  the Microsoft APIs Terms of Use. Push needs a public webhook, Event Hubs or
  Event Grid. Chaat refreshes when you look. For push, you run the small
  `chaat-relay` (in `relay/`) on any HTTPS host: it forwards Graph's
  content-free change notifications to Chaat ([details](docs/realtime.md)).
- **Consent.** Expect an administrator to approve Chaat's permissions in your
  tenant.

## Privacy

There is no telemetry and no Chaat server. Refresh tokens live in the OS
credential store. Your messages are cached only on your computer, and signing
out deletes them. More in [docs/privacy.md](docs/privacy.md).

## Development

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets     # unit, mock-Graph and headless UI tests
```

Read [AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md) first.
Report vulnerabilities privately through [SECURITY.md](SECURITY.md).

## Licence

MIT. See [LICENSE](LICENSE) and [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
All downloads include dependency, font and icon notices. `chaat --licenses`
and `chaat-relay --licenses` print the complete notices embedded in the binary.
The passphrase wordlist is the EFF large wordlist, copyright the Electronic
Frontier Foundation, under
[CC BY 3.0 US](https://creativecommons.org/licenses/by/3.0/us/).
Chaat is based on [Wren](https://github.com/MaxTheTurtle0/wren) by Andrei
Antonescu (MIT); the upstream copyright is kept in [LICENSE](LICENSE).
Chaat is an independent project, not affiliated with or endorsed by Microsoft.
Microsoft Teams is a trademark of the Microsoft group of companies.

