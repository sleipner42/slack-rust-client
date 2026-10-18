# Security

Please report vulnerabilities through
[GitHub private vulnerability reporting](https://github.com/sleipner42/slack-rust-client/security/advisories/new).
Include affected versions or commits, reproduction steps, and the expected
impact. Use fabricated tokens and messages in examples; do not send live
credentials, cookies, private workspace exports, or credential-store files.

Avoid public issues for vulnerabilities until a fix is available. This is a
small experimental project with no guaranteed response time; fixes target the
latest `main` branch rather than older releases.

## Data handling

The app uses the user's Slack session to call Slack APIs. Credentials and draft
text are stored locally as JSON; they are not encrypted by the app. New saves
use owner-only permissions. Draft text is also synced to Slack. Messages,
images, and videos are cached on disk. See the README for storage locations.

The repository and release bundles must never contain session data. Build DMGs
with `scripts/package-dmg.sh`, which stages a fresh app bundle from source.
Do not package a copy of your configuration or cache folders.

If a live credential is exposed, revoke the affected Slack session immediately.
Removing a file or rewriting Git history does not invalidate an exposed token.

## Dependency review

The 2026-10-18 review updates `rustls` to 0.23.46, addressing
RUSTSEC-2026-0346 through RUSTSEC-2026-0349. Keep `Cargo.lock` committed and
use `--locked` when building.

RustSec still reports upstream warnings that are not suppressed:

- `proc-macro-error`, `ttf-parser`, `zbus_names`, `zvariant`, and
  `zvariant_derive` are flagged as unmaintained.
- Linux's GTK dependency chain includes `glib` 0.18 with
  [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html),
  an unsound string-iterator implementation. The fix is in `glib` 0.20;
  updating that major version requires changes in the upstream GTK stack.

A passing build or an empty secret scan is not a security guarantee. These
dependency warnings remain follow-up work, especially for Linux releases.
