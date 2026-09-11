# Sift architecture (contributor overview)

- Tauri 2 + Rust core (tokio), React 19 in WKWebView.
- UI never blocks on network: SQLite (WAL, FTS5) is the truth for reads; outbox queue for writes.
- One writer (async write mutex + pool), keyset pagination, small IPC payloads, `sift-att://` for bytes.
- Gmail REST API transport; per-account SyncEngine/OutboxDrain/Backfill; 15s/60s poll.
- Sanitization once at ingest (ammonia + post-pass), sandboxed iframe render, remote images on by default.

See SIFT_SPEC.md Sections 5-9 for the full contract.
