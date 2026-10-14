# TODO

Keep the client compact, readable, and restrained, with an old IRC feel. Use the
existing workspace session for live checks; no demo mode.

## Next

- [ ] Validate scrolling and resizing in the release app with long conversations,
  image-heavy channels, long sidebar names, and an open thread. Automated layout
  regressions pass; check trackpad smoothness and idle battery use on the Mac.
- [ ] Verify unread indicators, the Unreads filter, and Dock badges against real
  incoming channel messages, mentions, and DMs, including muted conversations.
- [ ] Check the latest smaller type and consistent margins at narrow and wide
  window sizes, including the emoji picker and image viewer.
- [ ] Persist channel and thread drafts across restarts. Drafts currently survive
  switching conversations but are kept only in memory.
- [ ] Load older channel history when scrolling to the top, preserving the visible
  message position. Channel history currently fetches the latest 100 messages.

## Remaining features

- [ ] Expand Cmd+P search to all people and channels, beyond loaded conversations.
- [ ] Add native macOS notifications for mentions and DMs, respecting muted
  conversations and avoiding duplicates with the active conversation.
- [ ] Edit your own messages, with clear pending and failure states.
- [ ] Delete your own messages, with confirmation and failure recovery.
- [ ] Add byte-level upload progress, cancellation during upload, and a file chooser.

## Implemented

- [x] Paste images/screenshots and files copied in Finder into channel/thread
  drafts; thumbnails, filenames, removal, attachment-only sends, background
  uploads, and retries that preserve successful uploads and check ambiguous sends.
- [x] Slack Activity feed with DMs, mentions, threads and reactions; server unread
  filtering, cached previews, pagination, right-hand detail, and marking items read
  when opened, with retries if Slack rejects the update.
- [x] Shareable Apple Silicon DMG with a clean app bundle, Applications shortcut,
  ad-hoc signature and checksum. Developer ID notarization remains a release step.

- [x] Message formatting, quotes, code, emoji, and mentions.
- [x] Structured Slack message blocks and legacy attachments retain headings,
  code fences, indentation, and bot names/avatars instead of notification fallbacks.
- [x] Wrapped URLs share one hover highlight, tooltip and keyboard focus target;
  clicking any wrapped line opens the complete URL.
- [x] Message grouping, avatars, day separators, and timestamps.
- [x] Sidebar sections, unread emphasis, mention badges, muted styling, and
  recent DMs; a dedicated Unreads filter.
- [x] Aligned channel names, fixed icon columns, and truncated long names.
- [x] Multiline composer: Enter sends; Shift+Enter inserts a newline.
- [x] Focus the message composer after choosing a conversation with Cmd+P and keep
  it active after sending; immediate outgoing rows with pending and retry states.
- [x] Existing Slack To-dos with message previews, completed items, pagination,
  refresh, a message detail panel, copying full posts, selection with edge scrolling,
  and marking items completed in Slack.
- [x] Separate channel and thread drafts while the app is open.
- [x] Thread panel, reactions, emoji picker, and hover message controls.
- [x] Compact hover shortcuts for thumbs up, check mark and question mark reactions;
  selected reactions can be removed with another click.
- [x] Inline images and a click-to-open image modal.
- [x] Consistent gutters, smaller type, and an adaptive narrow-window thread view.
- [x] Custom macOS icon and Dock unread/mention badges.
- [x] Static loading placeholders and reserved image space to reduce layout jumps.
- [x] In-memory conversation cache and bounded, expiring disk history/image cache.
- [x] Show uncached history before profile/file metadata requests finish; persist
  profiles across launches, batch missing metadata, and preserve image slots.
- [x] GPU rendering, off-thread image decoding, bounded texture uploads,
  offscreen row culling, and repainting on demand.
- [x] Regression tests for channel-entry scrolling, resize layout, sidebar bounds,
  stale requests, drafts, unread state, and cached conversation switching.

Validation baseline: 68 tests pass; formatting and Clippy are clean. Performance
tests cover layout work, not live frame rate or measured energy consumption.
