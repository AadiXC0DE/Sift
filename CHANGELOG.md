# Changelog

## Unreleased

- Release Sift under the MIT License (`LICENSE`, plus `license` fields in `package.json` and `src-tauri/Cargo.toml`), and say so on the site: GitHub buttons in the navigation and hero, an "Open source" section on the landing page, license links in the footer of every page, download, FAQ, privacy, benchmarks, changelog and shortcuts pages, and `llms.txt`.
- Fix the mobile navigation bar: the download button drops its three-word label to "Download" below 640px so it no longer crowds the bar, and the GitHub button now appears in the navigation at every width (links were previously hidden below 960px, leaving phones with no route to the source).

## v1.1.1 (2026-09-14)

- Fix packaged email frames staying at 160px because the app CSP blocked their sizing script.
- Preserve authored email styles under the release CSP, including Tauri’s style nonce injection.
- Add WebKit and Chromium release-policy regression coverage for sizing, styles and quote collapse.
- Explain first launch before downloading, including the missing “Open Anyway” fallback.
- This beta remains unnotarized; updater signatures remain enforced.

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
  matrix plus artifact integrity and universality checks before publishing.
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

### Validation scope

Automated coverage includes account scoping, attachment transport, draft persistence,
undo and uncertain sends, search, privacy controls, and Chromium/WebKit rendering.
Native checks use a real Gmail mailbox. This does not certify every sender's HTML
or every live send scenario; no outgoing test email was sent during this review.
Apple Developer ID signing and notarization remain deferred for this release.
Measured performance and remaining measurement limits are recorded in `docs/perf.md`.

## v1.0.0 (2026-09-04)

- Initial release: multi-account Gmail, unified inbox, sync, reading, actions + undo, compose + drafts, search, palette, personalization, updater.
