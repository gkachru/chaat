# Roadmap

Roughly in order. Each step lands as its own focused change with tests.

## Next

1. **Windows and Linux visual pass.** Run on both, capture screenshots, fix
   platform chrome (title bar, fonts, tray). Known: on Windows the grey ✕ is
   hard to see on Close's red hover; `fastframe-titlebar`'s `caption_button`
   should draw it white while Close is hovered or pressed (upstream fix).
2. **Push relay deployment.** Publish a container image and deployment
   guide, add channel-post subscriptions.
3. **Upload progress** for large file attachments.

## Then

- Encrypt the cache (SQLCipher with a key in the OS credential store).
- Adaptive Cards: input fields and Submit (display of the common elements works; inputs and Submit open in Teams).
- Lazy per-script fallback fonts (upstream in fastframe-fonts), about 29 MB
  less memory.
- Pinned chats in the list, chat muting.
- Translations (fastframe-i18n).
- Signed and notarized macOS builds, a Windows installer, Flatpak and AUR.

## Calling

Calls run in Teams on the web beside Chaat ([calling.md](calling.md)).
Next:

1. Answer the [calling open questions](calling.md#open-questions)
   with a real browser, meeting and tenant: window placement with a running
   browser, the join page, silent sign-in, presence timing.
2. Watch for an official, documented API for in-call controls.

## Plugins

The plugin system is built: a plugin list in Settings › Plugins, one host
per plugin, capabilities routed to features (`call_alerts`, `presence_display`),
plugin-described options and presence for a status light
([plugins.md](plugins.md)).
Still open: a `commands` capability that lets a plugin ask Chaat to act, and
pinning plugin executables so a replaced file is noticed.
