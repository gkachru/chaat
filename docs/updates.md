# App updates

Official portable downloads and macOS app bundles can update themselves.
In Settings, the **Updates** section of **About** controls automatic checks and downloads and
provides **Check for updates**, **Download update**, and **Restart to update**.
A ready update also shows a persistent restart button below the workspace.

Automatic checks contact GitHub at most once a day, including across app
restarts. They use no Microsoft credentials or account data. Switching the
setting off stops future automatic checks and downloads; manual checking remains
available.

Chaat verifies an Ed25519 signature over the release's checksums, then verifies
the downloaded package against its checksum. The public key is compiled from
`packaging/update-public-key.hex`; no key supplied by a release is trusted.
The shared `fastframe-update` helper installs the update after Chaat exits,
relaunches it, and restores the previous version if the new window does not
acknowledge a successful start. macOS replaces the whole bundle and also
checks its identity and any existing Developer ID signing team.

Chaat never restarts automatically. Send or clear drafts and finish pending
sends before selecting **Restart to update**. This matters because drafts
and failed sends currently live in memory.

Package-manager installations must update through their package manager.
The `.deb` installs `/usr/bin/chaat` without the portable marker, so the
updater detects dpkg ownership and Chaat shows "Update this installation
through apt or your software center." when a newer release exists.
The updater also refuses unmarked Linux/Windows binaries and macOS copies
running from a disk image. Download and extract the official portable
archive, or move Chaat.app into Applications. The Linux installer adds the
portable marker beside its installed executable. Source builds and older
CI artifacts do not opt into automatic binary replacement.

Every archive and app bundle includes `LICENSE`, `THIRD-PARTY-NOTICES.md`,
`licenses/`, and the font and icon notices under `assets/`. Releases contain
only the app; the relay archive is a CI build artifact, not a release
package, and carries the same notice set. The Linux installer preserves it under
`$PREFIX/share/doc/chaat`, or `~/.local/share/doc/chaat` by default.
Notices are also embedded in the executables. Run `chaat --licenses` or
`chaat-relay --licenses` to read the current build's texts, including after a
Linux or Windows self-update that replaces only the executable.

Automatic updates need a release published by the release workflow below,
with a `checksums.txt.sig` signed by the key that matches
`packaging/update-public-key.hex`. Until one exists, Chaat reports the failed
check in Settings.

## Making a release

1. Keep Cargo.toml and Cargo.lock at the intended version. A release tag
   must match it exactly, such as `v0.4.0`.
2. Ensure the GitHub repository secret `UPDATE_SIGNING_KEY` holds the PEM
   Ed25519 private key matching `packaging/update-public-key.hex`. Keep a
   separate secure backup. Never commit the private key. Generate a pair
   with `openssl genpkey -algorithm ed25519 -out key.pem`; derive the hex
   file with `openssl pkey -in key.pem -pubout -outform DER | tail -c 32 |
   od -An -v -tx1 | tr -d ' \n'`.
   Releases do not include macOS yet: the release workflow builds Linux and
   Windows only until a Developer ID Application identity is set up. To add
   macOS, configure `MACOS_SIGNING_CERTIFICATE` with the base64 PKCS#12
   export of that certificate and private key and `MACOS_SIGNING_PASSWORD`
   with its export password, then restore the `macos-latest` entry and its
   signing steps in `.github/workflows/release.yml`. Release packaging never
   falls back to an ad-hoc signature.
3. Push the version tag. `.github/workflows/release.yml` runs format, lint,
   tests and the dependency/font notice check, then builds the Linux x86_64
   (a tarball and a `.deb` for Ubuntu 22.04+ and Debian 12+, built on
   `ubuntu-22.04`) and Windows x86_64
   packages. The `.deb` is covered by the signed checksums but has no
   self-update path; `packaging/linux/deb.sh` builds it locally. It checks that the signing key matches the compiled public key,
   signs `checksums.txt`, and creates a **draft** release.
4. Inspect the packages and release notes. On each platform, install the
   packages and check sign-in, a chat, and an update with a real account.
   Then publish the draft. This publication is what
   makes the update available to users.

Stable builds follow GitHub's latest stable release. Builds whose Cargo
version contains a prerelease suffix follow newer prereleases too.
macOS notarization and Windows publisher certificates remain separate from
the update-signing key. macOS release bundles require a persistent
`CODESIGN_IDENTITY`; ad-hoc signatures cannot preserve Keychain approval
across app updates. See [macOS signing](macos-keychain.md).

The package names, portable markers, version probe and restart protocol are
the shared fastframe-update contract. Keep `packaging/release.sh`, the
Windows packaging step, and the configuration in `src/updates.rs` together
when changing that contract. Do not replace the public key casually: existing
installations trust it and will refuse a release signed only by another key.
