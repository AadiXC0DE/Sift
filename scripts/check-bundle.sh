#!/usr/bin/env bash
# P10.2 release-size gate.
#
# Enforces, with exact measurements:
#   * eager JS gzip <= --max-js-kib (default 250 KiB), measured by walking the
#     Vite import manifest so every statically loaded chunk counts, not just a
#     file named "main";
#   * DMG <= --max-dmg-mib (default 12 MiB) against the signed universal
#     artifact (src-tauri/target/universal-apple-darwin/release).
#
# Modes:
#   --mode release  artifacts are required; a missing DMG fails the gate.
#   --mode debug    the DMG gate is skipped (debug bundles are never shipped),
#                   the eager-JS gate still runs.
#   --mode auto     release when the resolved target directory is a release
#                   layout, otherwise debug. This is the default.
#
# Flags (also accepted as --flag=value): --target-dir, --mode, --max-js-kib,
# --max-dmg-mib, --dist, --require-artifacts. Environment overrides:
# SIFT_TARGET_DIR, SIFT_BUNDLE_MODE, SIFT_MAX_JS_KIB, SIFT_MAX_DMG_MIB, SIFT_DIST.
set -euo pipefail

MODE=auto
TARGET_DIR=""
MAX_JS_KIB=250
MAX_DMG_MIB=12
DIST=dist
REQUIRE=0

usage() {
  sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    --mode=*) MODE="${1#*=}" ;;
    --mode) MODE="${2:?--mode needs a value}"; shift ;;
    --target-dir=*) TARGET_DIR="${1#*=}" ;;
    --target-dir) TARGET_DIR="${2:?--target-dir needs a value}"; shift ;;
    --max-js-kib=*) MAX_JS_KIB="${1#*=}" ;;
    --max-js-kib) MAX_JS_KIB="${2:?--max-js-kib needs a value}"; shift ;;
    --max-dmg-mib=*) MAX_DMG_MIB="${1#*=}" ;;
    --max-dmg-mib) MAX_DMG_MIB="${2:?--max-dmg-mib needs a value}"; shift ;;
    --dist=*) DIST="${1#*=}" ;;
    --dist) DIST="${2:?--dist needs a value}"; shift ;;
    --require-artifacts) REQUIRE=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "check-bundle: unknown argument: $1" >&2
      exit 2
      ;;
  esac
  shift
done

MODE="${SIFT_BUNDLE_MODE:-$MODE}"
TARGET_DIR="${SIFT_TARGET_DIR:-$TARGET_DIR}"
MAX_JS_KIB="${SIFT_MAX_JS_KIB:-$MAX_JS_KIB}"
MAX_DMG_MIB="${SIFT_MAX_DMG_MIB:-$MAX_DMG_MIB}"
DIST="${SIFT_DIST:-$DIST}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ -z "$TARGET_DIR" ]; then
  for candidate in \
    "$ROOT/src-tauri/target/universal-apple-darwin/release" \
    "$ROOT/src-tauri/target/release" \
    "$ROOT/src-tauri/target/debug"; do
    if [ -d "$candidate" ]; then
      TARGET_DIR="$candidate"
      break
    fi
  done
fi

if [ "$REQUIRE" = 1 ]; then
  MODE=release
fi

case "$MODE" in
  auto)
    if [ -n "$TARGET_DIR" ] && [ "$(basename "$TARGET_DIR")" = release ]; then
      MODE=release
    else
      MODE=debug
    fi
    ;;
  release | debug) ;;
  *)
    echo "check-bundle: --mode must be release, debug or auto (got '$MODE')" >&2
    exit 2
    ;;
esac

if [ -z "$TARGET_DIR" ]; then
  if [ "$MODE" = release ]; then
    echo "check-bundle: no target directory under src-tauri/target (release mode requires artifacts)" >&2
    exit 1
  fi
  TARGET_DIR="$ROOT/src-tauri/target/debug"
fi

file_bytes() {
  if [ "$(uname -s)" = Darwin ]; then
    stat -f %z "$1"
  else
    stat -c %s "$1"
  fi
}

mib() {
  awk -v b="$1" 'BEGIN { printf "%.2f", b / 1048576 }'
}

status=0
echo "bundle check mode=$MODE target=$TARGET_DIR max-js=${MAX_JS_KIB}KiB max-dmg=${MAX_DMG_MIB}MiB"

# --- eager JS gate (both modes) -------------------------------------------
if [ ! -d "$ROOT/$DIST" ]; then
  echo "FAIL no frontend build at $DIST (run 'pnpm build' first)" >&2
  status=1
elif ! node "$ROOT/scripts/eager-js.mjs" --dist "$ROOT/$DIST" --max-js-kib "$MAX_JS_KIB"; then
  echo "FAIL eager JS exceeds ${MAX_JS_KIB} KiB" >&2
  status=1
fi

# --- DMG gate (release only) ----------------------------------------------
dmg=""
if [ -d "$TARGET_DIR/bundle/dmg" ]; then
  shopt -s nullglob
  dmgs=("$TARGET_DIR"/bundle/dmg/*.dmg)
  shopt -u nullglob
  if [ "${#dmgs[@]}" -gt 0 ]; then
    dmg="${dmgs[0]}"
  fi
fi

if [ "$MODE" = release ]; then
  if [ -z "$dmg" ]; then
    echo "FAIL no DMG found in $TARGET_DIR/bundle/dmg (release mode requires the signed artifact)" >&2
    status=1
  else
    case "$TARGET_DIR" in
      *universal-apple-darwin*) ;;
      *) echo "note: DMG is not from the universal-apple-darwin layout: $dmg" >&2 ;;
    esac
    bytes="$(file_bytes "$dmg")"
    limit=$((MAX_DMG_MIB * 1048576))
    echo "dmg $(basename "$dmg") bytes=$bytes ($(mib "$bytes") MiB) gate=${MAX_DMG_MIB}MiB"
    if [ "$bytes" -gt "$limit" ]; then
      echo "FAIL DMG exceeds ${MAX_DMG_MIB} MiB" >&2
      status=1
    fi
  fi
else
  echo "dmg gate skipped (debug build is never shipped)"
fi

if [ "$status" -ne 0 ]; then
  echo "bundle check FAILED"
  exit 1
fi
echo "bundle check OK"
