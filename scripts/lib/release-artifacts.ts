// Release artifact discovery, latest.json / SHA256SUMS construction and
// verification. Shared by scripts/release-metadata.ts (real pipeline),
// scripts/updater-dryrun.ts (staged fixture endpoint) and the unit tests.
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import {
  PLATFORM_TARGETS,
  decodeSignatureBox,
  type PlatformTarget,
  type ReleaseManifest,
  publicKeyProblem,
  parseManifest,
  verifyArtifactSignature,
} from './updater';

export const DEFAULT_REPO = 'AadiXC0DE/Sift';
export const DEFAULT_CONFIG = 'src-tauri/tauri.conf.json';

export function sha256(data: Buffer): string {
  return createHash('sha256').update(data).digest('hex');
}

/**
 * Generated files must never carry private-key material. Minisign signature
 * boxes legitimately say "signature from tauri secret key", so only the
 * secret-key file markers themselves (and PEM blocks) count.
 */
export function secretMaterialProblem(text: string): string | null {
  if (/encrypted secret key|minisign secret key/i.test(text)) return 'generated output contains a minisign secret key';
  if (/-----BEGIN [A-Z ]*PRIVATE KEY-----/.test(text)) return 'generated output contains a PEM private key';
  return null;
}

export type BundleArtifacts = {
  dmg: string;
  archive: string;
  signatureFile: string;
  signatureBox: string;
};

function exactlyOne(dir: string, suffix: string, what: string): string {
  if (!existsSync(dir)) throw new Error(`${what}: directory does not exist: ${dir}`);
  const matches = readdirSync(dir)
    .filter((name) => name.endsWith(suffix))
    .map((name) => join(dir, name));
  if (matches.length === 0) throw new Error(`${what}: no *${suffix} under ${dir} (build with createUpdaterArtifacts)`);
  if (matches.length > 1) throw new Error(`${what}: ${matches.length} candidates under ${dir}: ${matches.join(', ')}`);
  return matches[0];
}

/** Locate the signed universal DMG and updater archive produced by `tauri build`. */
export function discoverBundle(bundleDir: string): BundleArtifacts {
  const dmg = exactlyOne(join(bundleDir, 'dmg'), '.dmg', 'DMG');
  const archive = exactlyOne(join(bundleDir, 'macos'), '.app.tar.gz', 'updater archive');
  const signatureFile = `${archive}.sig`;
  if (!existsSync(signatureFile)) throw new Error(`updater archive has no signature: ${signatureFile} is missing`);
  const signatureText = readFileSync(signatureFile, 'utf8').trim();
  // Tauri writes base64 of the minisign box; fixture/standalone minisign files
  // may contain the box itself. Normalize before encoding the manifest once.
  const signatureBox = signatureText.startsWith('untrusted comment:')
    ? signatureText
    : Buffer.from(signatureText, 'base64').toString('utf8');
  decodeSignatureBox(signatureBox);
  const secret = secretMaterialProblem(signatureBox);
  if (secret) throw new Error(`${signatureFile}: ${secret}`);
  return { dmg, archive, signatureFile, signatureBox };
}

export function assetUrl(repo: string, tag: string, name: string): string {
  return `https://github.com/${repo}/releases/download/${tag}/${name}`;
}

/**
 * Reject anything that is not an uploaded release asset: no website source
 * archives, no other tag, no other repository.
 */
export function urlProblem(url: string, expected: { repo: string; tag: string; name: string }): string | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return `not a url: ${url}`;
  }
  if (parsed.protocol !== 'https:' || parsed.hostname !== 'github.com') {
    return `must be an https://github.com url, got ${url}`;
  }
  if (parsed.pathname.includes('/archive/')) {
    return `points at a source archive instead of an uploaded asset: ${url}`;
  }
  const prefix = `/${expected.repo}/releases/download/${expected.tag}/`;
  if (!parsed.pathname.startsWith(prefix)) {
    return `must live under ${prefix}, got ${parsed.pathname}`;
  }
  const name = decodeURIComponent(parsed.pathname.slice(prefix.length));
  if (name !== expected.name) return `asset name ${name} does not match ${expected.name}`;
  return null;
}

/** Build the static latest.json the updater endpoint serves. */
export function buildManifest(input: {
  version: string;
  repo: string;
  tag: string;
  archiveName: string;
  signatureBox: string;
  pubDate: string;
  notes?: string;
}): ReleaseManifest {
  const url = assetUrl(input.repo, input.tag, input.archiveName);
  const signature = Buffer.from(input.signatureBox, 'utf8').toString('base64');
  const platforms: Record<string, { signature: string; url: string }> = {};
  for (const target of PLATFORM_TARGETS as readonly PlatformTarget[]) {
    platforms[target] = { signature, url };
  }
  return { version: input.version, notes: input.notes, pub_date: input.pubDate, platforms };
}

export type ChecksumRow = { sha256: string; name: string };

export function buildChecksums(rows: ChecksumRow[]): string {
  return (
    [...rows]
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((row) => `${row.sha256}  ${row.name}`)
      .join('\n') + '\n'
  );
}

export function parseChecksums(text: string): ChecksumRow[] {
  const rows: ChecksumRow[] = [];
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const match = /^([0-9a-f]{64})\s+\*?(.+)$/.exec(trimmed);
    if (!match) throw new Error(`SHA256SUMS: unparseable line: ${line}`);
    rows.push({ sha256: match[1], name: match[2] });
  }
  if (rows.length === 0) throw new Error('SHA256SUMS: no checksum rows');
  return rows;
}

export type VerifyAssetsInput = {
  /** directory holding latest.json, SHA256SUMS and the artifacts. */
  dir: string;
  repo: string;
  tag: string;
  pubkey: string;
  /** when set, checksums are recomputed from these files (remote mode). */
  files?: Record<string, Buffer>;
};

export type VerifyAssetsReport = { problems: string[]; manifest: ReleaseManifest | null; files: string[] };

/**
 * Validate generated metadata against the exact bytes that were (or will be)
 * uploaded: checksum rows are complete and correct, every platform url is a
 * release-download url, and each signature verifies against its archive.
 */
export function verifyAssets(input: VerifyAssetsInput): VerifyAssetsReport {
  const problems: string[] = [];
  const manifestText = readFileSync(join(input.dir, 'latest.json'), 'utf8');
  const sumsText = readFileSync(join(input.dir, 'SHA256SUMS'), 'utf8');

  const secret = secretMaterialProblem(manifestText);
  if (secret) problems.push(secret);

  let manifest: ReleaseManifest | null = null;
  try {
    manifest = parseManifest(JSON.parse(manifestText));
  } catch (err) {
    problems.push(`latest.json: ${(err as Error).message}`);
  }

  let rows: ChecksumRow[] = [];
  try {
    rows = parseChecksums(sumsText);
  } catch (err) {
    problems.push((err as Error).message);
  }
  const byName = new Map(rows.map((row) => [row.name, row.sha256]));
  const readAsset = (name: string): Buffer | null => {
    const fromMemory = input.files?.[name];
    if (fromMemory) return fromMemory;
    const localPath = join(input.dir, name);
    return existsSync(localPath) ? readFileSync(localPath) : null;
  };

  const files: string[] = [];
  for (const row of rows) {
    files.push(row.name);
    const bytes = readAsset(row.name);
    if (!bytes) {
      problems.push(`SHA256SUMS lists ${row.name} but it is not present`);
      continue;
    }
    const actual = sha256(bytes);
    if (actual !== row.sha256) problems.push(`${row.name}: sha256 ${actual} does not match published ${row.sha256}`);
  }
  if (input.files) {
    for (const name of Object.keys(input.files)) {
      // A checksum file cannot list itself.
      if (name === 'SHA256SUMS') continue;
      if (!byName.has(name)) problems.push(`${name} was uploaded but is missing from SHA256SUMS`);
    }
  }
  for (const name of ['latest.json']) {
    if (!byName.has(name)) problems.push(`SHA256SUMS is missing a row for ${name}`);
  }

  const archiveNames = new Set<string>();
  for (const [target, platform] of Object.entries(manifest?.platforms ?? {})) {
    const name = basename(new URL(platform.url).pathname);
    archiveNames.add(name);
    const urlProblemText = urlProblem(platform.url, { repo: input.repo, tag: input.tag, name });
    if (urlProblemText) problems.push(`platforms.${target}.url: ${urlProblemText}`);
    if (!byName.has(name)) {
      problems.push(`platforms.${target} points at ${name}, which SHA256SUMS does not cover`);
      continue;
    }
    const bytes = readAsset(name);
    if (!bytes) continue;
    const check = verifyArtifactSignature(input.pubkey, platform.signature, bytes);
    if (!check.ok) problems.push(`platforms.${target}.signature: ${check.reason}`);
  }
  if (archiveNames.size === 0) problems.push('latest.json has no platform urls');
  return { problems, manifest, files };
}

export function writeGenerated(outDir: string, manifest: ReleaseManifest, rows: ChecksumRow[]): void {
  writeFileSync(join(outDir, 'latest.json'), JSON.stringify(manifest, null, 2) + '\n');
  writeFileSync(join(outDir, 'SHA256SUMS'), buildChecksums(rows));
}

/** Key gate: refuse to produce metadata while the configured key is a placeholder. */
export function requireUsablePublicKey(configPath: string): string {
  const config: unknown = JSON.parse(readFileSync(configPath, 'utf8'));
  const pubkey =
    typeof config === 'object' && config !== null
      ? (config as { plugins?: { updater?: { pubkey?: unknown } } }).plugins?.updater?.pubkey
      : undefined;
  if (typeof pubkey !== 'string') throw new Error(`${configPath}: plugins.updater.pubkey is missing`);
  const problem = publicKeyProblem(pubkey);
  if (problem) {
    throw new Error(
      [
        `${configPath}: ${problem}.`,
        'The release would ship an app that cannot verify any update signature.',
        'Fix (one step):',
        '  pnpm tauri signer generate -w ~/.tauri/sift.key',
        '  pnpm exec tsx scripts/set-updater-pubkey.ts ~/.tauri/sift.key.pub',
        '  git add src-tauri/tauri.conf.json && git commit',
        'Store ~/.tauri/sift.key and its password as the TAURI_SIGNING_PRIVATE_KEY /',
        'TAURI_SIGNING_PRIVATE_KEY_PASSWORD repository secrets (never commit them).',
      ].join('\n'),
    );
  }
  return pubkey;
}
