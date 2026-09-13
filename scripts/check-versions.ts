// Release gate: the app version must agree across package.json, Cargo.toml,
// tauri.conf.json, CHANGELOG.md and (when supplied) the generated updater
// metadata and the release tag. Exits 1 on any mismatch.
//
// usage: pnpm exec tsx scripts/check-versions.ts [--tag v1.2.3] [--metadata latest.json]
import { readFileSync } from 'node:fs';
import { checkVersionConsistency } from './lib/version-sources';

function argValue(name: string): string | null {
  const index = process.argv.indexOf(name);
  if (index === -1) return null;
  const value = process.argv[index + 1];
  if (!value || value.startsWith('--')) {
    console.error(`${name} requires a value`);
    process.exit(2);
  }
  return value;
}

const tag = argValue('--tag');
const metadataPath = argValue('--metadata');
let metadataVersion: string | null = null;
if (metadataPath) {
  const parsed: unknown = JSON.parse(readFileSync(metadataPath, 'utf8'));
  if (typeof parsed !== 'object' || parsed === null) {
    console.error(`${metadataPath}: not a JSON object`);
    process.exit(2);
  }
  const version = (parsed as Record<string, unknown>).version;
  if (typeof version !== 'string') {
    console.error(`${metadataPath}: no version field`);
    process.exit(2);
  }
  metadataVersion = version;
}

const report = checkVersionConsistency({ tag, metadataVersion });
for (const entry of report.versions) {
  console.log(`${entry.version ?? '(missing)'}  ${entry.label}`);
}
console.log(`${report.changelog ? `v${report.changelog}` : '(missing)'}  CHANGELOG.md`);
if (metadataPath) console.log(`${metadataVersion ?? '(missing)'}  ${metadataPath}`);
if (tag) console.log(`${tag}  release tag`);
if (report.problems.length > 0) {
  console.error('version mismatch:');
  for (const problem of report.problems) console.error(`  - ${problem}`);
  console.error('run: pnpm exec tsx scripts/bump-version.ts <x.y.z> (then add the CHANGELOG heading)');
  process.exit(1);
}
console.log('version consistency ok');
