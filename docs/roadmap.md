# Roadmap

Roughly in order. Each step lands as its own focused change with tests.

## Next

1. **Linux visual pass.** Run on a Linux desktop, capture screenshots, fix
   platform chrome (title bar, fonts, tray). Known on Windows: with Chaat's
   own title bar (the default), the grey ✕ is hard to see on Close's red hover;
   `fastframe-titlebar`'s `caption_button` should draw it white while Close
   is hovered or pressed (upstream fix).
2. **Push relay deployment.** Publish a container image and deployment
   guide, add channel-post subscriptions.
3. **Upload progress** for large file attachments.

## Then

- Encrypt the cache (SQLCipher with a key in the OS credential store).
- Adaptive Cards: input fields and Submit (display of the common elements works; inputs and Submit open in Teams).
- Per-script fallback fonts: load only the script that visible text needs,
  instead of every fallback at once (upstream in fastframe-fonts).
- Pinned chats in the list.
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
per plugin, capabilities routed to features (`call_alerts`,
`presence_display`, `push`, `agents`), plugin-described options, presence for
a status light, and a pinned SHA-256 hash per plugin file so a replaced file
does not run until you approve it ([plugins.md](plugins.md)).
Still open: a `commands` capability that lets a plugin ask Chaat to act.
