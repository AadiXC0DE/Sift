// Keep package.json, Cargo.toml, tauri.conf.json versions equal.
// The source list lives in scripts/lib/version-sources.ts and is shared with
// scripts/check-versions.ts, so both agree on which files carry the version.
import { readFileSync } from 'node:fs';
import {
  CHANGELOG_PATH,
  VERSION_SOURCES,
  changelogVersion,
  parseVersion,
  writeSourceVersion,
} from './lib/version-sources';

const v = process.argv[2];
if (!v) {
  console.error('usage: bump-version <x.y.z>');
  process.exit(1);
}
try {
  parseVersion(v);
} catch (err) {
  console.error(`bump-version: ${(err as Error).message}`);
  process.exit(1);
}
for (const source of VERSION_SOURCES) {
  writeSourceVersion(source, v);
  console.log(`wrote ${source.path}`);
}
const documented = changelogVersion(readFileSync(CHANGELOG_PATH, 'utf8'));
if (documented !== v) {
  console.log(`${CHANGELOG_PATH}: add a "## v${v}" heading (currently ${documented ? `v${documented}` : 'missing'})`);
}
console.log(`bumped to ${v}`);
