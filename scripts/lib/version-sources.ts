// Single source of truth for the files that must agree on the app version.
// `bump-version.ts` writes through this list and `check-versions.ts` validates it,
// so a new version location is added once and both stay in sync.
import { readFileSync, writeFileSync } from 'node:fs';

export type VersionSource = {
  label: string;
  path: string;
  extract: (raw: string) => string | null;
  replace: (raw: string, version: string) => string;
};

export const VERSION_SOURCES: VersionSource[] = [
  {
    label: 'package.json',
    path: 'package.json',
    extract: (raw) => {
      const parsed: unknown = JSON.parse(raw);
      if (typeof parsed !== 'object' || parsed === null) return null;
      const version = (parsed as Record<string, unknown>).version;
      return typeof version === 'string' ? version : null;
    },
    replace: (raw, version) => {
      const parsed = JSON.parse(raw) as Record<string, unknown>;
      parsed.version = version;
      return JSON.stringify(parsed, null, 2) + '\n';
    },
  },
  {
    label: 'src-tauri/Cargo.toml',
    path: 'src-tauri/Cargo.toml',
    extract: (raw) => /^version\s*=\s*"([^"]+)"/m.exec(raw)?.[1] ?? null,
    replace: (raw, version) => raw.replace(/^version\s*=\s*".*"/m, `version = "${version}"`),
  },
  {
    label: 'src-tauri/tauri.conf.json',
    path: 'src-tauri/tauri.conf.json',
    extract: (raw) => {
      const parsed: unknown = JSON.parse(raw);
      if (typeof parsed !== 'object' || parsed === null) return null;
      const version = (parsed as Record<string, unknown>).version;
      return typeof version === 'string' ? version : null;
    },
    replace: (raw, version) => raw.replace(/"version":\s*".*?"/, `"version": "${version}"`),
  },
];

export const CHANGELOG_PATH = 'CHANGELOG.md';

/** First `## vX.Y.Z` heading in the changelog (any trailing date note is ignored). */
export function changelogVersion(raw: string): string | null {
  return /^##\s+v(\d+\.\d+\.\d+)\b/m.exec(raw)?.[1] ?? null;
}

export function readSourceVersion(source: VersionSource): { version: string | null; raw: string } {
  const raw = readFileSync(source.path, 'utf8');
  return { version: source.extract(raw), raw };
}

export function writeSourceVersion(source: VersionSource, version: string): void {
  const { raw } = readSourceVersion(source);
  writeFileSync(source.path, source.replace(raw, version));
}

export type ConsistencyInput = {
  /** version from a generated artifact (latest.json); checked when present. */
  metadataVersion?: string | null;
  /** release tag, e.g. `v1.2.3`; checked when present. */
  tag?: string | null;
  changelogPath?: string;
};

export type ConsistencyReport = {
  versions: { label: string; version: string | null }[];
  changelog: string | null;
  problems: string[];
};

const SEMVER_RE = /^(\d+)\.(\d+)\.(\d+)(?:[-+]([0-9A-Za-z.-]+))?$/;

export function parseVersion(value: unknown): [number, number, number] {
  if (typeof value !== 'string') throw new Error('version is not a string');
  const match = SEMVER_RE.exec(value.trim());
  if (!match) throw new Error(`version ${JSON.stringify(value)} is not x.y.z semver`);
  return [Number(match[1]), Number(match[2]), Number(match[3])];
}

/** -1 / 0 / 1, ignoring prerelease ordering flags beyond the numeric triple. */
export function compareVersions(a: string, b: string): number {
  const left = parseVersion(a);
  const right = parseVersion(b);
  for (let i = 0; i < 3; i += 1) {
    if (left[i] !== right[i]) return left[i] < right[i] ? -1 : 1;
  }
  return 0;
}

export function checkVersionConsistency(input: ConsistencyInput = {}): ConsistencyReport {
  const versions = VERSION_SOURCES.map((source) => ({ label: source.label, version: readSourceVersion(source).version }));
  const changelog = changelogVersion(readFileSync(input.changelogPath ?? CHANGELOG_PATH, 'utf8'));
  const problems: string[] = [];

  for (const entry of versions) {
    if (!entry.version) problems.push(`${entry.label}: no version found`);
  }
  const present = versions.filter((entry) => entry.version).map((entry) => entry.version as string);
  const expected = present[0];
  for (const entry of versions) {
    if (entry.version && entry.version !== expected) {
      problems.push(`${entry.label} is ${entry.version} but ${versions[0].label} is ${expected}`);
    }
  }
  if (!changelog) problems.push(`${input.changelogPath ?? CHANGELOG_PATH}: no "## vX.Y.Z" release heading`);
  if (changelog && expected && changelog !== expected) {
    problems.push(`${input.changelogPath ?? CHANGELOG_PATH} documents v${changelog} but ${versions[0].label} is ${expected}`);
  }
  if (input.metadataVersion && expected && input.metadataVersion !== expected) {
    problems.push(`generated metadata version ${input.metadataVersion} does not match ${versions[0].label} ${expected}`);
  }
  if (input.tag) {
    const tagVersion = input.tag.replace(/^v/, '');
    if (expected && tagVersion !== expected) {
      problems.push(`release tag ${input.tag} does not match ${versions[0].label} ${expected}`);
    }
  }
  return { versions, changelog, problems };
}
