#!/bin/bash
# P11-T22: IMAP connection budget. At most 2 established connections per
# account when the app is idle (3 are allowed during a foreground transfer).
#
# Usage: scripts/check-connections.sh <pid> <expected_accounts>
#
# This is a runtime check against a *live signed-in* app: it reads the
# established TCP connections to imap.gmail.com from lsof. Two honest limits:
#
#   * lsof cannot attribute a connection to an account, so the assertion is on
#     the total (2 x expected_accounts). Give the real account count.
#   * The benchmark fixture used by scripts/bench-start.sh / bench-mem.sh /
#     bench-idle-cpu.sh is deliberately credential-free, so no socket is ever
#     opened there and this check cannot be exercised from the bench harness.
#     It needs a real signed-in app on a live network.
#
# Exit status: 0 within budget, 1 on breach, 2 when it cannot measure.
set -euo pipefail

PID="${1:-}"
ACCOUNTS="${2:-}"

if [ -z "$PID" ]; then
  echo "usage: $0 <pid> <expected_accounts>" >&2
  exit 2
fi
if [ -z "$ACCOUNTS" ]; then
  echo "check-connections: expected_accounts is required; the budget is per account" >&2
  exit 2
fi
if ! command -v lsof >/dev/null 2>&1; then
  echo "check-connections: lsof unavailable; not measured" >&2
  exit 2
fi
if ! ps -p "$PID" >/dev/null 2>&1; then
  echo "check-connections: no process $PID; not measured" >&2
  exit 2
fi

CONNS=$(lsof -nP -p "$PID" -a -i TCP -sTCP:ESTABLISHED 2>/dev/null | grep -c "imap.gmail.com" || true)
LIMIT=$((2 * ACCOUNTS))

echo "check-connections: pid $PID established imap.gmail.com connections: $CONNS"
echo "check-connections: budget <= 2 x $ACCOUNTS account(s) = $LIMIT"
if [ "$CONNS" -gt "$LIMIT" ]; then
  echo "check-connections: BREACH"
  exit 1
fi
echo "check-connections: PASS"
