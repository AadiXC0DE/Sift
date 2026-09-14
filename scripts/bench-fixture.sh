#!/bin/bash
# Build an isolated, hermetic benchmark data directory from a snapshot of a real
# mailbox.
#
# The app is pointed at this directory with SIFT_DATA_DIR, so no benchmark ever
# reads or writes the mailbox at
#   ~/Library/Application Support/com.aadixc0de.sift
# The directory is a *copy* of the pre-migration snapshot, so the first launch
# after `--fresh` also exercises the schema 6 -> current migration (a real cost,
# reported rather than hidden). The copy is an APFS clone (`cp -c`): instant,
# copy-on-write, and the source file is never modified.
#
# Two edits make the copy safe to launch repeatedly and free of network work,
# which the launch budget requires ("no network dependency"):
#
#   1. `DELETE FROM outbox_ops`. The real mailbox's outbox held an `inflight`
#      modify_labels row; the app retries in-flight work at startup, which would
#      issue a real STORE against the user's Gmail account from a benchmark.
#   2. Account e-mails are rewritten to `bench+<id>@invalid.test`. OAuth
#      credentials live in the macOS keychain keyed by *e-mail*
#      (src-tauri/src/secrets.rs), so no credential resolves for the rewritten
#      address and `provider_for` fails before any socket is opened. Accounts,
#      threads and messages stay in place, so the Inbox still renders the real
#      43,000-message local state; only sync is inert.
#
# Usage: scripts/bench-fixture.sh [--fresh]
#   --fresh  rebuild the directory even if it already exists
# Prints the data directory path on stdout.
#
# Env:
#   SIFT_BENCH_DATA_DIR    target directory (default: $TMPDIR/sift-bench-data)
#   SIFT_BENCH_SOURCE_DB   source snapshot (default: the premigration backup)
set -euo pipefail
cd "$(dirname "$0")/.."

BENCH_DIR="${SIFT_BENCH_DATA_DIR:-${TMPDIR:-/tmp}/sift-bench-data}"
SRC="${SIFT_BENCH_SOURCE_DB:-$HOME/Library/Application Support/com.aadixc0de.sift.premigration-backup/sift.db}"
FRESH=0
for arg in "$@"; do
  case "$arg" in
    --fresh) FRESH=1 ;;
    *) echo "bench-fixture: unknown argument: $arg" >&2; exit 2 ;;
  esac
done

# Refuse to operate on anything that looks like real user data.
case "$BENCH_DIR" in
  *bench*) ;;
  *) echo "bench-fixture: refusing: $BENCH_DIR does not look like a benchmark directory" >&2; exit 2 ;;
esac
if [ "$BENCH_DIR" = "$HOME/Library/Application Support/com.aadixc0de.sift" ]; then
  echo "bench-fixture: refusing to use the real data directory" >&2
  exit 2
fi

if [ -f "$BENCH_DIR/sift.db" ] && [ "$FRESH" = 0 ]; then
  echo "$BENCH_DIR"
  exit 0
fi

rm -rf "$BENCH_DIR"
mkdir -p "$BENCH_DIR"

if [ -f "$SRC" ]; then
  cp -c "$SRC" "$BENCH_DIR/sift.db" 2>/dev/null || cp "$SRC" "$BENCH_DIR/sift.db"
  sqlite3 "$BENCH_DIR/sift.db" <<'SQL'
DELETE FROM outbox_ops;
UPDATE accounts SET email = 'bench+' || id || '@invalid.test';
SQL
  echo "bench-fixture: cloned $SRC -> $BENCH_DIR/sift.db (outbox cleared, credentials detached)" >&2
else
  echo "bench-fixture: no source snapshot at $SRC; the app will create a fresh database" >&2
fi

echo "$BENCH_DIR"
