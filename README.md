# Better Slack

A compact, unofficial Slack client for macOS and Linux, written in Rust with
egui and wgpu. Inspired by old IRC clients: readable, restrained, and fast.
This is an independent project, not affiliated with or endorsed by Slack.

- Channels, DMs, threads, reactions, inline images, and unread indicators.
- Activity feed with mentions, replies, reactions, and read-on-open behavior.
- Slack to-dos with a detail panel and completion controls.
- Cmd+P to switch conversations; Enter to send, Shift+Enter for a new line.
- Local caching and immediate outgoing messages.
- Message editing/sharing, English and Swedish spell checking, and emoji completion.
- Draft text saved across restarts and synced with Slack between devices.
- Cmd+V to attach images or files copied in Finder, in channels and threads.
  Preview/remove attachments before sending (up to 10 files, 100 MB total).

## Run

Requires a recent stable Rust toolchain and CMake (which builds the Opus audio
codec for huddles), plus Xcode Command Line Tools on macOS (`brew install cmake`
for CMake).

```sh
git clone https://github.com/sleipner42/slack-rust-client.git
cd slack-rust-client
cargo run --release --locked
```

On Linux, install `pkg-config` and the development packages for ALSA, D-Bus,
GTK 3, WebKitGTK 4.1, and libsoup 3 (e.g. `libasound2-dev libdbus-1-dev
libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev` on Debian/Ubuntu). On NixOS, run the
commands inside `nix-shell`. Ctrl replaces Cmd in shortcuts.

Uses your existing local Slack desktop session or opens Slack sign-in. On
Linux, a keyring-encrypted session needs a running Secret Service (e.g. GNOME
Keyring or KeePassXC). Credentials and cached messages stay in
your user config and cache directories, outside this repository.
Some features use internal Slack APIs and may change without notice.

## Local data and privacy

Session credentials and draft text are stored as local JSON, not in the OS
keychain. New saves use owner-only file permissions; disk encryption remains
the responsibility of your operating system. Draft text also syncs to Slack;
unsent attachment data is kept in memory and does not survive restarts.

App data lives under `~/Library/Application Support/better-slack` on macOS
or `${XDG_CONFIG_HOME:-~/.config}/better-slack` on Linux. Cached messages,
images, and videos live under `~/Library/Caches/better-slack` or
`${XDG_CACHE_HOME:-~/.cache}/better-slack`. These folders are separate from
the repository. Do not include them in bug reports or shared app bundles.

Report security issues [privately](SECURITY.md). For ordinary bugs, open a
GitHub issue with reproduction steps and your OS version, removing workspace
content, credentials, and private URLs from any logs or screenshots.

## Build and check

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
bash scripts/package-macos.sh  # target/Better Slack.app
bash scripts/package-dmg.sh    # target/dist/
```

The current DMG build targets Apple Silicon and is ad-hoc signed, not notarized.
Older channel pagination is planned. CI runs formatting, Clippy, and tests on
macOS and Linux. CI uses synthetic test data and does not sign in to Slack.
See [TODO.md](TODO.md) and [third-party notices](THIRD_PARTY.md).
