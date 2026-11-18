#!/bin/sh
# Installs Chaat for the current user: the binary in ~/.local/bin, the
# desktop entry and icons under ~/.local/share.
#
#   packaging/linux/install.sh [path/to/chaat]
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
if [ -f "$here/chaat" ]; then
    binary="${1:-$here/chaat}"
    icon="$here/chaat.svg"
    notices="$here"
else
    binary="${1:-$here/../../target/release/chaat}"
    icon="$here/../icons/chaat.svg"
    notices="$here/../.."
fi
prefix="${PREFIX:-$HOME/.local}"

# Check before replacing an installation. Both source and archive installs
# retain the notices alongside the installed binary's documentation.
test -s "$notices/licenses/dependencies.txt"
test -s "$notices/LICENSE"
test -s "$notices/THIRD-PARTY-NOTICES.md"
documentation="$prefix/share/doc/chaat"
mkdir -p "$documentation/assets/fonts" "$documentation/assets/icons"
install -m644 "$notices/LICENSE" "$notices/THIRD-PARTY-NOTICES.md" "$documentation/"
cp -R "$notices/licenses" "$documentation/"
install -m644 "$notices/assets/fonts/Inter-LICENSE.txt" "$documentation/assets/fonts/"
install -m644 "$notices/assets/icons/LICENSE.txt" "$documentation/assets/icons/"
install -Dm755 "$binary" "$prefix/bin/chaat"
# Marks this user-owned copy for verified self-updates. Package managers
# never run this installer and remain responsible for their own updates.
printf '%s\n' chaat-portable-v1 > "$prefix/bin/chaat-portable.txt"
install -Dm644 "$here/chaat.desktop" "$prefix/share/applications/chaat.desktop"
install -Dm644 "$here/chaat-256.png" "$prefix/share/icons/hicolor/256x256/apps/chaat.png"
install -Dm644 "$icon" "$prefix/share/icons/hicolor/scalable/apps/chaat.svg"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$prefix/share/applications" || true
fi
echo "Installed Chaat to $prefix/bin/chaat"
