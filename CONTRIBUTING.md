# Contributing

Thanks for helping. Read [AGENTS.md](AGENTS.md) first: it holds the product
boundaries (official APIs only, no polling, least privilege) and the
architecture rules.

## Setup

- Rust 1.98 or newer (`rust-toolchain.toml` pins it for rustup).
- Linux: `libxkbcommon-dev libwayland-dev libgl1-mesa-dev` (Debian and
  Ubuntu names), plus a Secret Service provider (GNOME Keyring or KWallet)
  for signing in.
- macOS: Xcode command line tools.
- Windows: the MSVC toolchain.

```sh
cargo run                  # real account (see docs/azure-app-registration.md)
```

## Checks

Run before every push:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps
```

The tests need no network and no account. Graph is a local `wiremock` server,
and the interface runs headless over the demo data.

CI also scans the complete Git history with Gitleaks and checks `Cargo.lock`
against RustSec advisories with `cargo audit`. Both checks run weekly as well
as on pushes and pull requests. Dependabot proposes Cargo dependency updates
weekly and GitHub Actions updates monthly.

When changing dependencies, regenerate the committed license notices:

```sh
cargo install cargo-about --locked --version 0.9.2
packaging/generate-notices.sh
packaging/generate-notices.sh --check
```

The report covers the client and relay on all packaged targets. CI and
release verification check it against `Cargo.lock`. Keep the supplemental
font and icon licenses described in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Keep every fastframe crate on the same tag. Dependabot groups their updates
into one pull request so internal dependencies use the same revision too.

CI runs on pushes to `main` and on pull requests: format, lint and docs
(with a check that the license notices are current), tests on Linux, macOS
and Windows, and the macOS signing regression. Pushes to `main` also build
packages for all three platforms. The security workflow adds the secret scan
and the dependency audit. History stays linear: pull requests are squash-merged, with no merge
commits.

Keep local environment files, signing keys, account databases, and logs out
of commits. Sanitised `.env.example` files are allowed. Report suspected
vulnerabilities through [SECURITY.md](SECURITY.md).

Set `CODESIGN_IDENTITY` to the same Developer ID or Apple Development identity
for every signed-in build. For offline demo screenshots only, set
`CODESIGN_IDENTITY=-` explicitly. Ad-hoc signatures change identity on rebuild
and cause recurring Keychain prompts. See [macOS signing](docs/macos-keychain.md).
`packaging/macos/test-signing.sh` checks identity stability on macOS with
synthetic certificates in a temporary keychain and no account credentials.

Brand artwork lives in `packaging/icons/`: `chaat.svg` is the bowl-only app
icon, `tray-template.svg` is the transparent monochrome bowl, and
`chaat-wordmark.svg` contains the bowl and name. The white wordmark variant
supports dark backgrounds and is tinted by the welcome screen's theme.
After changing the artwork, regenerate the checked-in platform icons:

```sh
cargo run --locked --example render-icon -- packaging/linux/chaat-256.png 256
cargo run --locked --example render-icon -- packaging/windows/chaat.ico
```

The macOS bundler generates its icon from the same SVG.

## Pull requests

- One topic per pull request, with tests for changed behaviour.
- For interface changes, attach before and after screenshots in both themes.
- Do not add Graph permissions, background fetching or new dependencies
  without explaining why in the pull request.
