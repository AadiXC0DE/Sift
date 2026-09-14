#!/bin/bash
# Release gates that must pass before a draft release is published.
#
#   bash scripts/verify-release.sh --mode strict --bundle-dir <dir>   # release workflow
#   bash scripts/verify-release.sh --selftest                         # no signed artifacts needed
#   bash scripts/verify-release.sh --rollback-check --repo owner/name --tag vX.Y.Z
#
# Strict mode fails on a missing/unsigned/un-notarized app or a missing
# artifact. Nothing is swallowed with `|| true`: every check reports ok/FAIL and
# any FAIL exits non-zero. Local mode is for inspecting a debug build and prints
# a loud banner; the release workflow never uses it.
set -uo pipefail

MODE=strict
BUNDLE_DIR=src-tauri/target/universal-apple-darwin/release/bundle
REPO=AadiXC0DE/Sift
TAG=""
SELFTEST=0
ROLLBACK=0

usage() {
  echo "usage: verify-release.sh [--mode strict|unnotarized|local] [--bundle-dir DIR] [--repo owner/name] [--tag vX.Y.Z] [--selftest] [--rollback-check]" >&2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --mode) MODE="${2:?}"; shift 2 ;;
    --bundle-dir) BUNDLE_DIR="${2:?}"; shift 2 ;;
    --repo) REPO="${2:?}"; shift 2 ;;
    --tag) TAG="${2:?}"; shift 2 ;;
    --selftest) SELFTEST=1; shift ;;
    --rollback-check) ROLLBACK=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

FAILURES=0
SKIPPED=0

ok() { echo "ok    $1"; }
fail() { echo "FAIL  $1"; FAILURES=$((FAILURES + 1)); }
skip() { echo "skip  $1"; SKIPPED=$((SKIPPED + 1)); }

run_check() {
  local label="$1"
  shift
  if "$@" >/dev/null 2>&1; then ok "$label"; else fail "$label"; fi
}

selftest() {
  echo "selftest: release tooling without signed artifacts"
  if pnpm exec tsx scripts/check-versions.ts; then ok "version consistency"; else fail "version consistency"; fi
  if pnpm exec tsx scripts/updater-dryrun.ts; then ok "updater failure-mode dry-run"; else fail "updater failure-mode dry-run"; fi
  exit $((FAILURES > 0))
}

if [ "$SELFTEST" = "1" ]; then selftest; fi

package_version() {
  node -p "require('./package.json').version"
}

rollback_check() {
  echo "rollback installer check (previous published release must still carry a DMG)"
  if ! command -v gh >/dev/null 2>&1; then
    skip "gh not installed; cannot inspect previous releases"
    return
  fi
  local tags
  if ! tags=$(gh release list --repo "$REPO" --limit 100 --json tagName,isDraft --jq '.[] | select(.isDraft == false) | .tagName' 2>/dev/null); then
    fail "gh could not list releases for $REPO (missing auth?)"
    return
  fi
  if [ -z "$tags" ]; then
    ok "no published release yet, so there is no rollback installer to verify"
    return
  fi
  local previous
  previous=$(printf '%s\n' "$tags" | grep -v "^${TAG}$" | sort -V | tail -1)
  if [ -z "$previous" ]; then
    ok "no earlier published release than $TAG"
    return
  fi
  local assets
  assets=$(gh release view "$previous" --repo "$REPO" --json assets --jq '.assets[].name' 2>/dev/null)
  if printf '%s\n' "$assets" | grep -q '\.dmg$' && printf '%s\n' "$assets" | grep -q '^SHA256SUMS$'; then
    ok "$previous still publishes a DMG and SHA256SUMS (rollback installer)"
  else
    fail "$previous has no DMG/SHA256SUMS left; users cannot roll back after a bad upgrade"
  fi
}

if [ "$ROLLBACK" = "1" ]; then
  rollback_check
  exit $((FAILURES > 0))
fi

if [ "$MODE" = "local" ]; then
  echo "======================================================================"
  echo "LOCAL MODE: signature and notarization gates are SKIPPED."
  echo "This is not a release gate; CI uses --mode strict."
  echo "======================================================================"
fi

if [ "$MODE" != "strict" ] && [ "$MODE" != "unnotarized" ] && [ "$MODE" != "local" ]; then
  usage; exit 2
fi
if [ "$MODE" = "unnotarized" ]; then
  echo "UNNOTARIZED RELEASE: Apple Developer ID/notarization deferred; updater signatures remain required."
fi
VERSION=$(package_version)
APP="$BUNDLE_DIR/macos/Sift.app"
DMG=$(ls "$BUNDLE_DIR"/dmg/*.dmg 2>/dev/null | head -1)
ARCHIVE=$(ls "$BUNDLE_DIR"/macos/*.app.tar.gz 2>/dev/null | head -1)
SIGNATURE="${ARCHIVE}.sig"

if [ -z "$DMG" ]; then
  if [ "$MODE" != "local" ]; then fail "no DMG under $BUNDLE_DIR/dmg"; else skip "no DMG (local build without bundling)"; fi
else
  ok "DMG present: $(basename "$DMG")"
fi
if [ -z "$ARCHIVE" ]; then
  if [ "$MODE" != "local" ]; then fail "no *.app.tar.gz updater archive under $BUNDLE_DIR/macos"; else skip "no updater archive"; fi
else
  ok "updater archive present: $(basename "$ARCHIVE")"
  if [ -f "$SIGNATURE" ]; then ok "updater signature present: $(basename "$SIGNATURE")"; else fail "missing $SIGNATURE"; fi
  if tar -tzf "$ARCHIVE" 2>/dev/null | grep '^Sift.app/Contents/MacOS/sift$' >/dev/null; then
    ok "archive contains Sift.app/Contents/MacOS/sift"
  else
    fail "archive does not contain the Sift executable"
  fi
fi

if [ ! -d "$APP" ]; then
  if [ "$MODE" != "local" ]; then fail "no app bundle at $APP"; else skip "no app bundle to sign-check"; fi
else
  if [ "$MODE" = "strict" ]; then
    run_check "codesign --verify --deep --strict" codesign --verify --deep --strict --verbose=2 "$APP"
    if spctl -a -vvv -t exec "$APP" 2>&1 | grep -q 'accepted'; then ok "spctl accepts the app (notarized Developer ID)"; else fail "spctl rejected the app"; fi
    run_check "stapler validates the app" xcrun stapler validate "$APP"
    if [ -n "$DMG" ]; then
      run_check "stapler validates the DMG" xcrun stapler validate "$DMG"
      if spctl -a -t open --context context:primary-signature -vvv "$DMG" 2>&1 | grep -q 'accepted'; then
        ok "spctl accepts the DMG"
      else
        fail "spctl rejected the DMG"
      fi
    fi
  elif [ "$MODE" = "unnotarized" ]; then
    run_check "ad-hoc code signature integrity" codesign --verify --deep --strict --verbose=2 "$APP"
    skip "Developer ID/notarization (explicit unnotarized release)"
  else
    skip "codesign/spctl/stapler (local mode)"
  fi
  ARCHS=$(lipo -archs "$APP/Contents/MacOS/sift" 2>/dev/null || echo "")
  if printf '%s' "$ARCHS" | grep -q 'x86_64' && printf '%s' "$ARCHS" | grep -q 'arm64'; then
    ok "universal binary: $ARCHS"
  else
    fail "app binary is not universal (archs: ${ARCHS:-unknown})"
  fi
  BUNDLE_VERSION=$(plutil -extract CFBundleShortVersionString raw -o - "$APP/Contents/Info.plist" 2>/dev/null || echo "")
  if [ "$BUNDLE_VERSION" = "$VERSION" ]; then ok "app bundle version $BUNDLE_VERSION matches package.json"; else fail "app bundle version ${BUNDLE_VERSION:-missing} does not match package.json $VERSION"; fi
fi

echo "----"
echo "verify-release: $FAILURES failure(s), $SKIPPED skipped (mode=$MODE)"
[ "$FAILURES" -eq 0 ]
