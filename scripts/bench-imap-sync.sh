#!/bin/bash
# P11-T23: IMAP sync bench against the seeded benchmark account (19.5 methodology).
#
# Stated budgets: first 200 threads <= 10 s; full metadata <= 90 s; quiet poll
# <= 300 ms (median of 20 ticks).
#
# NOT MEASURED HERE. The two cargo targets below exercise the sync code paths
# against wiremock/offline fixtures and emit no timings; the budgets are defined
# against the seeded *live* benchmark account
# (scripts/seed-benchmark-account.ts, Gmail OAuth, 20k messages / 8k threads)
# and a network this repo cannot reproduce offline. Running this script proves
# the sync paths still work; it does not measure, and must not be read as, the
# P11-T23 numbers. See docs/perf.md for the budget table and its status.
#
# Usage: scripts/bench-imap-sync.sh
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== imap full sync (offline fixture) =="
cargo test --manifest-path src-tauri/Cargo.toml --test imap_full_sync -- --nocapture 2>&1 | tail -n 6

echo "== quiet poll: p11_t06_new_mail_notifies (idempotent second tick) =="
cargo test --manifest-path src-tauri/Cargo.toml --test imap_partial_sync p11_t06_new_mail_notifies -- --nocapture 2>&1 | tail -n 4

cat <<'MSG'
bench-imap-sync: the sync paths above pass or fail on their own assertions.
bench-imap-sync: P11-T23 budgets (first 200 threads <=10 s, full metadata <=90 s,
bench-imap-sync: quiet poll <=300 ms median of 20) are NOT measured by this
bench-imap-sync: script: no test emits a timing, and the seeded live benchmark
bench-imap-sync: account is not available offline. Status in docs/perf.md.
MSG
