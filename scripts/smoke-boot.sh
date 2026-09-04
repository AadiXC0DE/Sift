#!/bin/bash
# Boot smoke test: catches startup-only crashes (Tauri config deserialization,
# plugin init) that cargo test can never see. Runs the debug binary headless
# of vite (the window shows an error page, but Rust setup must still run) and
# asserts the window-shown marker with no panic.
#
# Usage: ./scripts/smoke-boot.sh
# Fails the run on: panic in output, missing perf:window-shown marker, timeout.
set -e
cd "$(dirname "$0")/.."

echo "smoke-boot: building debug binary..."
cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tail -n 1

BIN=src-tauri/target/debug/sift
LOG=$(mktemp /tmp/sift-smoke.XXXXXX.log)
echo "smoke-boot: launching (20s budget)..."
(set -o pipefail; timeout -s KILL 20 "$BIN" >"$LOG" 2>&1 || true)

if grep -q "panicked\|PluginInitialization" "$LOG"; then
  echo "smoke-boot: FAIL — panic during startup:"
  grep -B1 -A6 "panicked\|PluginInitialization" "$LOG" | head -n 30
  exit 1
fi

if ! grep -q "perf:window-shown" "$LOG"; then
  echo "smoke-boot: FAIL — no perf:window-shown marker (setup did not complete):"
  head -n 30 "$LOG"
  exit 1
fi

echo "smoke-boot: PASS — window shown, no panics"
rm -f "$LOG"
