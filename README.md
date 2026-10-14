# Better Slack

A compact, unofficial Slack client for macOS and Linux, written in Rust with
egui and wgpu. Inspired by old IRC clients: readable, restrained, and fast.

- Channels, DMs, threads, reactions, inline images, and unread indicators.
- Activity feed with mentions, replies, reactions, and read-on-open behavior.
- Slack to-dos with a detail panel and completion controls.
- Cmd+P to switch conversations; Enter to send, Shift+Enter for a new line.
- Local caching and immediate outgoing messages.

## Run

Requires a recent stable Rust toolchain and Xcode Command Line Tools on macOS.

```sh
cargo run --release --locked
```

On Linux, install `pkg-config` and the development packages for D-Bus, GTK 3,
WebKitGTK 4.1, and libsoup 3 (e.g. `libdbus-1-dev libgtk-3-dev
libwebkit2gtk-4.1-dev libsoup-3.0-dev` on Debian/Ubuntu). On NixOS, run the
commands inside `nix-shell`. Ctrl replaces Cmd in shortcuts.

Uses your existing local Slack desktop session or opens Slack sign-in. On
Linux, a keyring-encrypted session needs a running Secret Service (e.g. GNOME
Keyring or KeePassXC). Credentials and cached messages stay in
your user config and cache directories, outside this repository.
Some features use internal Slack APIs and may change without notice.

## Build and check

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
bash scripts/package-macos.sh  # target/Better Slack.app
bash scripts/package-dmg.sh    # target/dist/
```

The current DMG build targets Apple Silicon and is ad-hoc signed, not notarized.
Drafts are kept only while the app is open; older channel pagination and file
uploads are planned. See [TODO.md](TODO.md) and [third-party notices](THIRD_PARTY.md).
