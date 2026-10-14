# Better Slack

A compact, unofficial Slack client for macOS, written in Rust with egui and
Metal rendering. Inspired by old IRC clients: readable, restrained, and fast.

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

Uses your existing local Slack session or opens Slack sign-in. Credentials and
cached messages stay in your macOS user directories, outside this repository.
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
