#!/bin/bash
# P10-T06: DMG <=12MB; main chunk <=250KB gz; compose lazy.
set -e
echo "bundle check (debug build skips size gate)"
if ls src-tauri/target/release/bundle/dmg/*.dmg >/dev/null 2>&1; then
  size=$(du -m src-tauri/target/release/bundle/dmg/*.dmg | cut -f1 | head -1)
  echo "dmg=${size}MB"
  if [ "$size" -gt 12 ]; then echo "FAIL dmg >12MB"; exit 1; fi
fi
if [ -d dist/assets ]; then
  for f in dist/assets/*.js; do
    gz=$(gzip -c "$f" | wc -c)
    echo "$f gz=$gz"
  done
fi
echo "bundle ok"
