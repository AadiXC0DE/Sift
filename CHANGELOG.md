# Changelog

## v1.1.0 (2026-09-14)

- Sync recipientless drafts without treating them as failed sends or connection failures.
- Avoid startup IMAP lock inversion and drain queued changes before refreshing the mailbox.
- Align composer buttons, keep connection warnings readable and remove the title-bar inset in full screen.
- Preserve authored email body styles and size the reader when quoted content collapses.
- Quieter account markers and aligned unread indicators without an extra blue focus stripe.
- Settings → Updates: check, download, verify, install and restart with explicit controls.
- This release is not Apple Developer ID signed or notarized. Updater signatures remain enforced.

### Reliability hardening

### What changed

- **IMAP attachment transport.** The section FETCH is emitted as
  `(UID BODY.PEEK[<section>])`, dotted sections are used for nested parts and
  root section `1` for single-part messages, and the strict fake Gmail server
  rejects the previously malformed form. Downloads resolve the message's real
  identity (Gmail message id plus UIDVALIDITY epoch) instead of trusting a stale
  cached UID, and a disconnect after `SELECT` reselects the mailbox before the
  next command.
- **Attachment lifecycle.** Attachments carry an account-qualified identity, a
  cache state machine with metadata-then-body enrichment, collision-safe
  filenames, atomic streaming writes that never leave a half file or a false
  `ready` state, repair of a deleted cache file, corrupt-payload refetch, disk
  full and read-only destination handling, Save All, and a confirmation step
  before handing a downloaded executable to the system opener.
- **Account-scoped storage and IPC (migration 0008).** Messages, labels, bodies
  and attachments are keyed by account with real foreign keys; orphaned rows go
  into a recovery report rather than being discarded; FTS is rebuilt afterwards;
  inline images resolve through `sift-att://<account>/<message>/<key>` with an
  ownership check; removing an account cleans every scoped table.
- **Connection and migration integrity.** PRAGMAs, including `foreign_keys`, are
  applied by the SQLite pool's per-connection setup hook so every pooled
  connection behaves the same; migrations run on a dedicated bootstrap
  connection, each inside `BEGIN IMMEDIATE` with the version update in the same
  transaction, preceded by a `VACUUM INTO` snapshot under `backups/`; and
  `write_tx` exists for genuinely atomic multi-statement work.
- **Cancellable account work.** Each account's loops are owned by a generation
  with a cancellation token, so removal stops them and awaits them within a
  bound before secrets and rows are deleted, late network results are dropped,
  and re-authentication keeps the mailbox and drafts.
- **Thread list, reader and navigation.** Account-qualified reading and actions,
  a 1000-row window with page eviction, density-aware row heights, and a row
  marker that does not form a multi-row stripe between adjacent rows.
- **Composer.** Attachment staging reports per-file success and failure instead
  of swallowing errors, the native drag/drop listener registers only while the
  composer is mounted and only accepts drops inside it, the fake `attachHelper`
  module is gone, a queued autosave is flushed when the sheet unmounts, and
  recipient chips, Cc/Bcc-only sends, formatting and signature handling are
  complete.
- **Size and release gates.** `ComposerSheet`, `SettingsDialog`, the palette and
  shortcut help are lazy chunks behind an enforced eager-JS ceiling
  (`scripts/eager-js.mjs`, 250 KiB gzip; this branch measures 187.7 KiB), the
  release workflow generates `latest.json` and `SHA256SUMS` from the exact
  staged bytes, verifies the uploaded bytes while the release is still a draft,
  refuses a placeholder updater key, and re-runs the 13-scenario updater failure
  matrix plus strict codesign/notarization/universality checks before publishing.
- **Settings → Storage.** `storage_usage` and `storage_clear_attachment_cache`
  are registered Rust commands; the cache cap defaults to 512 MiB.
- **Browser test suite.** The Playwright suite drives the real frontend against a
  deterministic fixture backend on chromium and webkit, and fails on unexpected
  console errors, unhandled rejections or commands the fixture does not
  implement, instead of asserting that `#root` exists.
- **Documentation.** README and `docs/architecture.md` no longer quote unmeasured
  performance or size figures, `docs/release.md` describes the current schema
  version and the upgrade path that actually exists, and the new
  `docs/acceptance-dossier.md` records what is proven, what is pending and what
  is not measured.

### What is not verified

This branch is **not** live-acceptance proof for attachments. The following are
open, and the dossier in `docs/acceptance-dossier.md` lists each one:

- **Live Gmail.** No real mailbox has been used. The P2.8 protocol has not been
  run: *implementation verified offline; live attachment acceptance pending.*
  The same applies to the outgoing send round trip, which also depends on the
  MIME fixes below.
- **Native and measured evidence.** No native screenshots, no checksum table, no
  benchmark report, and no measurement of launch time, idle memory, CPU, latency
  or per-account connection counts. Only the eager-JS ceiling and the DMG
  ceiling have executable gates.
- **Known defects still present.** Undo inverts the queued change instead of
  restoring a recorded previous state and cannot undo a permanent delete;
  permanent delete never reaches the server because the local rows are removed
  before the outbox op can read them; an interrupted send is requeued as
  `pending` at startup, so it can be sent twice; there is no guard that refuses a
  database written by a newer app; the Snoozed view orders by `snoozed_until`
  while its keyset cursor is `last_message_at`, so paging it can skip or repeat
  rows; `in:` parses but is never applied by local search; compose still emits
  `From: me` with unwrapped base64 attachment bodies and no `In-Reply-To`; and
  search has no local test coverage at all.
- **Privacy.** Remote images load by default (`remoteImages: "always"`). The
  blocked mode drops remote sources from the frame's CSP, but no test asserts
  that a blocked message makes zero outbound requests.
- **CI parity.** The Playwright config declares chromium and webkit for every
  run, while CI installs only chromium.

## v1.0.0 (2026-09-04)

- Initial release: multi-account Gmail, unified inbox, sync, reading, actions + undo, compose + drafts, search, palette, personalization, updater.
