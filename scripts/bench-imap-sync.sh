#!/bin/bash
# P11-T23: IMAP sync bench against the seeded 2k fixture (19.5 methodology).
# Budgets: first 200 threads ≤10 s; full metadata ≤90 s; quiet poll ≤300 ms (median of 20).
set -euo pipefail
cd "$(dirname "$0")/.."
echo "== imap full sync (2k fixture) =="
cargo test --manifest-path src-tauri/Cargo.toml --test imap_full_sync -- --nocapture 2>&1 | tail -n 6
echo "== quiet poll p11_t06_new_mail second tick (idempotent, cheap) =="
cargo test --manifest-path src-tauri/Cargo.toml --test imap_partial_sync p11_t06_new_mail_notifies -- --nocapture 2>&1 | tail -n 4
echo "bench-imap-sync ok (see docs/perf.md for release-build numbers)"
