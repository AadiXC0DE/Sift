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
  by background sync. This is what keeps interactions under the budget.
- **Optimistic writes.** An action mutates SQLite immediately, enqueues an
  outbox op, and reconciles with the server later. Undo cancels queued ops.
- **One writer.** All SQLite writes go through a single write lane; reads use a
  connection pool. Keyset pagination keeps list queries cheap.
- **Small IPC payloads.** List rows are paged at 100 and never include bodies.
  Attachment and inline-image bytes move over the `sift-att://` scheme, not JSON.
- **Faithful mail rendering.** HTML is sanitized once at ingest, then rendered in
  a sandboxed iframe with the sender's CSS, tables, and layout preserved. Remote
  images load by default and can be blocked in Settings.

## Transports

Gmail is reached two ways:

- **IMAP/SMTP with an app password** (the default for public builds). No Google
  Cloud project or OAuth verification is needed.
- **Gmail REST API over OAuth** (optional, for builds that ship a client id).

Both write the same local rows, so the rest of the app does not care which one is
in use. See [`oauth.md`](oauth.md) if you want to enable the Google sign-in
button.

## Startup

The Rust `setup` hook opens the database, sets the window background to match the
resolved theme (dark, light, or the OS appearance), then starts the supervisor:
one poll loop, outbox drain, and body backfill per account, plus a snooze
watcher. The frontend shows a small themed splash until React paints the shell.
