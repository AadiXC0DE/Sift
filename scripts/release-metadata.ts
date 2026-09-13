// Produce and verify the release metadata the updater endpoint serves.
//
//   pnpm exec tsx scripts/release-metadata.ts \
//     --bundle-dir src-tauri/target/universal-apple-darwin/release/bundle \
//     --out-dir release-assets --tag "$GITHUB_REF_NAME" --repo "$GITHUB_REPOSITORY"
//
//   pnpm exec tsx scripts/release-metadata.ts --verify release-assets
//   pnpm exec tsx scripts/release-metadata.ts --verify-remote --tag v1.2.3
//
// Generation refuses to run while tauri.conf.json still holds the placeholder
// updater key: shipping metadata for an app that cannot verify a signature is
// worse than shipping nothing. No private key is read, written or printed.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import {
  DEFAULT_CONFIG,
  DEFAULT_REPO,
  buildChecksums,
  buildManifest,
  discoverBundle,
  requireUsablePublicKey,
  secretMaterialProblem,
  sha256,
  urlProblem,
  verifyAssets,
} from './lib/release-artifacts';
import { verifyArtifactSignature } from './lib/updater';
import { checkVersionConsistency, readSourceVersion, VERSION_SOURCES } from './lib/version-sources';

const DEFAULT_BUNDLE_DIR = 'src-tauri/target/universal-apple-darwin/release/bundle';
const DEFAULT_OUT_DIR = 'release-assets';

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

function hasFlag(name: string): boolean {
  return process.argv.includes(name);
}

function fail(message: string): never {
  console.error(`release-metadata: ${message}`);
  process.exit(1);
}

function packageVersion(): string {
  const source = VERSION_SOURCES.find((entry) => entry.path === 'package.json');
  if (!source) fail('package.json is missing from VERSION_SOURCES');
  const version = readSourceVersion(source).version;
  if (!version) fail(`${source.path}: no version`);
  return version;
}

const configPath = argValue('--config') ?? DEFAULT_CONFIG;
const repo = argValue('--repo') ?? DEFAULT_REPO;
const tag = argValue('--tag') ?? `v${packageVersion()}`;
const pubkey = requireUsablePublicKey(configPath);
const verifyDir = argValue('--verify');

function assertVersionsMatch(metadataVersion: string | null): void {
  const report = checkVersionConsistency({ tag, metadataVersion });
  if (report.problems.length > 0) {
    fail(`version consistency:\n  - ${report.problems.join('\n  - ')}`);
  }
}

function generate(): void {
  const version = packageVersion();
  assertVersionsMatch(null);
  const bundleDir = argValue('--bundle-dir') ?? DEFAULT_BUNDLE_DIR;
  const outDir = argValue('--out-dir') ?? DEFAULT_OUT_DIR;
  const notesPath = argValue('--notes');
  const notes = notesPath ? readFileSync(notesPath, 'utf8').trim() : undefined;

  const artifacts = discoverBundle(bundleDir);
  const archiveBytes = readFileSync(artifacts.archive);
  const dmgBytes = readFileSync(artifacts.dmg);
  const signatureBytes = readFileSync(artifacts.signatureFile);
  const signatureField = Buffer.from(artifacts.signatureBox, 'utf8').toString('base64');
  const signatureCheck = verifyArtifactSignature(pubkey, signatureField, archiveBytes);
  if (!signatureCheck.ok) fail(`updater archive signature rejected: ${signatureCheck.reason}`);

  const manifest = buildManifest({
    version,
    repo,
    tag,
    archiveName: basename(artifacts.archive),
    signatureBox: artifacts.signatureBox,
    pubDate: argValue('--pub-date') ?? new Date().toISOString(),
    notes,
  });
  const manifestText = JSON.stringify(manifest, null, 2) + '\n';
  mkdirSync(outDir, { recursive: true });
  // out-dir becomes the exact staging set that gets uploaded, so the checksums
  // and the bytes they describe always travel together.
  copyFileSync(artifacts.dmg, join(outDir, basename(artifacts.dmg)));
  copyFileSync(artifacts.archive, join(outDir, basename(artifacts.archive)));
  copyFileSync(artifacts.signatureFile, join(outDir, basename(artifacts.signatureFile)));
  writeFileSync(join(outDir, 'latest.json'), manifestText);

  const rows = [
    { name: basename(artifacts.dmg), sha256: sha256(dmgBytes) },
    { name: basename(artifacts.archive), sha256: sha256(archiveBytes) },
    { name: basename(artifacts.signatureFile), sha256: sha256(signatureBytes) },
    { name: 'latest.json', sha256: sha256(Buffer.from(manifestText, 'utf8')) },
  ];
  const sumsText = buildChecksums(rows);
  const secret = secretMaterialProblem(manifestText + sumsText);
  if (secret) fail(secret);
  writeFileSync(join(outDir, 'SHA256SUMS'), sumsText);

  // Verify the generated rows against the exact bytes staged for upload.
  const report = verifyAssets({ dir: outDir, repo, tag, pubkey });
  if (report.problems.length > 0) fail(`generated metadata failed verification:\n  - ${report.problems.join('\n  - ')}`);

  console.log(`latest.json v${version} -> ${tag} (${repo})`);
  for (const [target, platform] of Object.entries(manifest.platforms)) {
    console.log(`  ${target}: ${platform.url}`);
  }
  for (const row of rows) console.log(`  ${row.sha256}  ${row.name}`);
  console.log(`updater signature: ok, ${rows.length} checksum rows in ${join(outDir, 'SHA256SUMS')}`);
}

function verifyLocal(): void {
  const parsed: unknown = JSON.parse(readFileSync(join(verifyDir as string, 'latest.json'), 'utf8'));
  const metadataVersion =
    typeof parsed === 'object' && parsed !== null && typeof (parsed as Record<string, unknown>).version === 'string'
      ? ((parsed as Record<string, unknown>).version as string)
      : null;
  assertVersionsMatch(metadataVersion);
  const report = verifyAssets({ dir: verifyDir as string, repo, tag, pubkey });
  if (report.problems.length > 0) fail(`metadata verification failed:\n  - ${report.problems.join('\n  - ')}`);
  console.log(`verified latest.json + SHA256SUMS in ${verifyDir} (${report.files.length} files)`);
}

type ReleaseAsset = { name: string; url: string; state: string; size: number };

function ghJson(args: string[]): unknown {
  const output = execFileSync('gh', args, { encoding: 'utf8', env: process.env });
  return JSON.parse(output);
}

function verifyRemote(): void {
  const localDir = argValue('--out-dir') ?? DEFAULT_OUT_DIR;
  const local = verifyAssets({ dir: localDir, repo, tag, pubkey });
  if (local.problems.length > 0) fail(`local metadata failed verification:\n  - ${local.problems.join('\n  - ')}`);
  const expected = new Set([...local.files, 'SHA256SUMS']);

  const release = ghJson(['release', 'view', tag, '--repo', repo, '--json', 'isDraft,assets']);
  if (typeof release !== 'object' || release === null) fail(`gh returned no release for ${tag}`);
  const { isDraft, assets } = release as { isDraft?: unknown; assets?: unknown };
  if (isDraft !== true) fail(`release ${tag} is not a draft; refusing to verify a published release in place`);
  if (!Array.isArray(assets)) fail(`release ${tag} has no asset list`);

  const seen = new Set<string>();
  for (const asset of assets as ReleaseAsset[]) {
    seen.add(asset.name);
    if (!expected.has(asset.name)) fail(`release ${tag} carries unexpected asset ${asset.name}`);
    if (asset.state !== 'uploaded') fail(`${asset.name} is in state ${asset.state}, not uploaded`);
    const problem = urlProblem(asset.url, { repo, tag, name: asset.name });
    if (problem) fail(`${asset.name}: ${problem}`);
  }
  for (const name of expected) {
    if (!seen.has(name)) fail(`release ${tag} is missing ${name}`);
  }

  const downloadDir = mkdtempSync(join(tmpdir(), 'sift-release-'));
  execFileSync('gh', ['release', 'download', tag, '--repo', repo, '--dir', downloadDir, '--clobber'], {
    encoding: 'utf8',
    env: process.env,
  });
  const downloaded: Record<string, Buffer> = {};
  for (const name of expected) downloaded[name] = readFileSync(join(downloadDir, name));

  const remote = verifyAssets({ dir: downloadDir, repo, tag, pubkey, files: downloaded });
  if (remote.problems.length > 0) fail(`downloaded assets failed verification:\n  - ${remote.problems.join('\n  - ')}`);
  const localVersion = local.manifest?.version;
  if (remote.manifest?.version !== localVersion) {
    fail(`downloaded latest.json is v${remote.manifest?.version} but local metadata is v${localVersion}`);
  }
  for (const [target, platform] of Object.entries(remote.manifest?.platforms ?? {})) {
    console.log(`  ${target}: ${platform.url} (bytes + signature verified)`);
  }
  console.log(`verified ${expected.size} uploaded assets for draft ${tag} against SHA256SUMS`);
}

if (hasFlag('--verify-remote') && verifyDir) fail('use either --verify <dir> or --verify-remote, not both');
if (hasFlag('--verify-remote')) {
  verifyRemote();
} else if (verifyDir) {
  verifyLocal();
} else {
  generate();
}
