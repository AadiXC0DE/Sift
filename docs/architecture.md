# Architecture

A short orientation for anyone working on the code. The UI never blocks on the
network; everything the user sees is read from a local database.

## Layout

- `src-tauri/` — Rust core: sync engines, MIME parsing, HTML sanitizing, search
  indexing, the outbox, and the OS integrations.
- `src/` — React 19 frontend rendered in the macOS WebView.
- `sift-site/` — the public site (`usesift.xyz`), built with Astro.

## Core ideas

- **Local-first reads.** Every mailbox is mirrored into SQLite (WAL, FTS5). List
  queries, search, and thread opens read from disk; the network is only touched
  by background sync, so an interaction never waits on a socket. The latency
  budgets themselves are measured on device, not in this document: the release
  budget rows and their current state live in
  [`acceptance-dossier.md`](acceptance-dossier.md).
- **Optimistic writes.** An action mutates SQLite immediately, enqueues an
  outbox op, and reconciles with the server later. Undo cancels queued ops and
  inverts the ones already applied; it is an inversion of the queued change, not
  a restore of a recorded previous state, so a permanent delete is not undoable.
- **One write lane.** Every SQLite write goes through `Db::write`, which holds
  the write lock; `Db::write_tx` wraps multi-statement work in `BEGIN
  IMMEDIATE` so it lands as one transaction. Reads use a four-connection pool,
  and the PRAGMAs (including `foreign_keys`) are applied by the pool's
  per-connection setup hook, so every pooled connection behaves the same.
  Keyset pagination keeps list queries cheap, and the frontend caps the loaded
  window at 1000 rows and evicts pages behind it.
- **Small IPC payloads.** List rows are paged at 100 and never include bodies.
  Attachment bytes move over account-qualified
  `sift-att://<account>/<message>/<key>` URLs rather than JSON, and inline
  images referenced by a message are embedded as `data:` URIs inside the
  sandboxed frame's document.
- **Faithful mail rendering.** HTML is sanitized once at ingest, then rendered in
  a sandboxed iframe with the sender's CSS, tables, and layout preserved. Remote
  images load by default and can be blocked in Settings. Blocking drops
  `http`/`https` sources from the frame's CSP; the resulting absence of outbound
  requests is a native check, not something the current test suite asserts.

## Transports

Gmail is reached two ways:

- **IMAP/SMTP with an app password** (the default for public builds). No Google
  Cloud project or OAuth verification is needed.
- **Gmail REST API over OAuth** (optional, for builds that ship a client id).

Both are wired into production paths and write the same local rows, so the rest
of the app does not care which one is in use. An IMAP account holds at most
three connections: the sync worker, the IDLE slot, and one bounded foreground
lease that user-initiated fetches borrow and release after a short idle period.
Some older comments inside `src-tauri/src/provider/imap/` still describe the
provider as unwired and the connection count as two; they are stale, and the
follow-up list in [`acceptance-dossier.md`](acceptance-dossier.md) records them
with line numbers.

See [`oauth.md`](oauth.md) if you want to enable the Google sign-in button.

## Startup

The Rust `setup` hook opens the database (bootstrap connection first, then the
pool), sets the window background to match the resolved theme (dark, light, or
the OS appearance), then starts the supervisor: one poll loop, outbox drain, and
body backfill per account, plus a snooze watcher. Per-account work is owned by a
generation with a cancellation token, so removing an account stops its loops and
awaits them before its secrets and rows are deleted. The frontend shows a small
themed splash until React paints the shell.
