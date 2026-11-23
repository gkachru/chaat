#!/bin/bash
# Builds dist/chaat_<version>-1_amd64.deb for Ubuntu 22.04+ and Debian 12+.
# Links against glibc 2.35 with cargo-zigbuild, so it runs on any Linux host
# (or WSL). CI builds on ubuntu-22.04 and calls `cargo deb` directly.
# Needs: rustup, cargo-zigbuild, zig, cargo-deb, objdump.
#
#   packaging/linux/deb.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
target=x86_64-unknown-linux-gnu
cargo zigbuild --release --locked --target "$target.2.35" --bin chaat
cargo deb --no-build --locked --target "$target" --output dist/
binary="${CARGO_TARGET_DIR:-target}/$target/release/chaat"
ceiling="$(objdump -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"
echo "Highest glibc symbol version needed: $ceiling"
if [ "$(printf '%s\nGLIBC_2.35\n' "$ceiling" | sort -V | tail -1)" != GLIBC_2.35 ]; then
    echo "The binary needs more than glibc 2.35." >&2
    exit 1
fi
ls -l dist/chaat_*_amd64.deb
