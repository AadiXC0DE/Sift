// End-to-end checks for scripts/release-metadata.ts against a synthetic,
// fixture-signed bundle. No real signing key or release artifact is involved.
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { makeFixtureKey, signBox, signatureField, type FixtureKey } from './fixture-signing';
import { parseChecksums } from './release-artifacts';
import { verifyArtifactSignature } from './updater';

const REPO = 'AadiXC0DE/Sift';
const TAG = 'v1.0.0';
const ARCHIVE = 'Sift.app.tar.gz';

function run(args: string[], env: Record<string, string> = {}): { status: number; stdout: string; stderr: string } {
  try {
    return {
      status: 0,
      stdout: execFileSync('pnpm', ['exec', 'tsx', 'scripts/release-metadata.ts', ...args], {
        encoding: 'utf8',
        env: { ...process.env, ...env },
      }),
      stderr: '',
    };
  } catch (err) {
    const failure = err as { status?: number; stdout?: string; stderr?: string };
    return { status: failure.status ?? 1, stdout: failure.stdout ?? '', stderr: failure.stderr ?? '' };
  }
}

/**
 * Stub `gh` so the post-upload verification runs without GitHub: `release view`
 * returns a fixture release, `release download` copies the fixture assets.
 */
function stubGh(dir: string, release: unknown, assets: Record<string, Buffer>): { bin: string; env: Record<string, string> } {
  const bin = join(dir, 'stub-bin');
  const fixture = join(dir, 'stub-gh');
  mkdirSync(join(fixture, 'assets'), { recursive: true });
  writeFileSync(join(fixture, 'release.json'), JSON.stringify(release));
  for (const [name, bytes] of Object.entries(assets)) writeFileSync(join(fixture, 'assets', name), bytes);
  mkdirSync(bin, { recursive: true });
  writeFileSync(
    join(bin, 'gh'),
    [
      '#!/bin/bash',
      'if [ "$1" = "release" ] && [ "$2" = "view" ]; then cat "$GH_FIXTURE/release.json"; exit 0; fi',
      'if [ "$1" = "release" ] && [ "$2" = "download" ]; then',
      '  while [ $# -gt 0 ]; do if [ "$1" = "--dir" ]; then dir="$2"; fi; shift; done',
      '  cp "$GH_FIXTURE"/assets/* "$dir"/; exit 0',
      'fi',
      'echo "stub gh: unsupported $*" >&2; exit 1',
      '',
    ].join('\n'),
    { mode: 0o755 },
  );
  return { bin, env: { PATH: `${bin}:${process.env.PATH ?? ''}`, GH_FIXTURE: fixture } };
}

function releaseAssets(files: Record<string, Buffer>): { name: string; url: string; state: string; size: number }[] {
  return Object.entries(files).map(([name, bytes]) => ({
    name,
    url: `https://github.com/${REPO}/releases/download/${TAG}/${name}`,
    state: 'uploaded',
    size: bytes.length,
  }));
}

type FixtureBundle = { dir: string; config: string; bundle: string; archive: Buffer; key: FixtureKey };

function fixtureBundle(): FixtureBundle {
  const dir = mkdtempSync(join(tmpdir(), 'sift-release-test-'));
  const bundle = join(dir, 'bundle');
  mkdirSync(join(bundle, 'dmg'), { recursive: true });
  mkdirSync(join(bundle, 'macos'), { recursive: true });
  const archive = gzipSync(Buffer.from('fixture universal app archive'));
  const key = makeFixtureKey();
  writeFileSync(join(bundle, 'dmg', `Sift_${TAG.slice(1)}_universal.dmg`), Buffer.from('fixture dmg bytes'));
  writeFileSync(join(bundle, 'macos', ARCHIVE), archive);
  writeFileSync(join(bundle, 'macos', `${ARCHIVE}.sig`), signBox(key, archive, ARCHIVE));
  return { dir, config: join(dir, 'tauri.conf.json'), bundle, archive, key };
}

function writeConfig(path: string, pubkey: string): void {
  writeFileSync(path, JSON.stringify({ plugins: { updater: { active: true, pubkey } } }));
}

describe('release-metadata pipeline', () => {
  it('generates latest.json and SHA256SUMS that verify, and rejects a tampered archive', () => {
    const fixture = fixtureBundle();
    writeConfig(fixture.config, fixture.key.pubkeyField);
    const archive = fixture.archive;
    const out = join(fixture.dir, 'out');

    const generated = run([
      '--bundle-dir', fixture.bundle,
      '--out-dir', out,
      '--config', fixture.config,
      '--repo', REPO,
      '--tag', TAG,
      '--pub-date', '2026-09-13T00:00:00Z',
    ]);
    expect(generated.stderr).toBe('');
    expect(generated.status).toBe(0);
    expect(generated.stdout).toContain(`latest.json v1.0.0 -> ${TAG}`);

    const manifest: unknown = JSON.parse(readFileSync(join(out, 'latest.json'), 'utf8'));
    const parsed = manifest as { version: string; platforms: Record<string, { signature: string; url: string }> };
    expect(parsed.version).toBe('1.0.0');
    expect(Object.keys(parsed.platforms).sort()).toEqual(['darwin-aarch64', 'darwin-universal', 'darwin-x86_64']);
    for (const platform of Object.values(parsed.platforms)) {
      expect(platform.url).toBe(`https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}`);
      expect(verifyArtifactSignature(fixture.key.pubkeyField, platform.signature, archive)).toEqual({ ok: true });
    }
    

    const rows = parseChecksums(readFileSync(join(out, 'SHA256SUMS'), 'utf8'));
    expect(rows.map((row) => row.name).sort()).toEqual(
      [`Sift_1.0.0_universal.dmg`, ARCHIVE, `${ARCHIVE}.sig`, 'latest.json'].sort(),
    );

    const verified = run(['--verify', out, '--config', fixture.config, '--repo', REPO, '--tag', TAG]);
    expect(verified.stderr).toBe('');
    expect(verified.status).toBe(0);

    // A byte flip after generation must be caught by the same verification.
    writeFileSync(join(out, ARCHIVE), Buffer.concat([archive, Buffer.from('tampered')]));
    const tampered = run(['--verify', out, '--config', fixture.config, '--repo', REPO, '--tag', TAG]);
    expect(tampered.status).toBe(1);
    expect(tampered.stderr).toContain('sha256');
  }, 60_000);

  it('refuses to generate metadata while the updater key is a placeholder', () => {
    const fixture = fixtureBundle();
    writeConfig(fixture.config, Buffer.from('untrusted comment: replace with real updater public key', 'utf8').toString('base64'));
    const result = run(['--bundle-dir', fixture.bundle, '--out-dir', join(fixture.dir, 'out'), '--config', fixture.config, '--repo', REPO, '--tag', TAG]);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('placeholder');
    expect(result.stderr).toContain('pnpm tauri signer generate -w ~/.tauri/sift.key');
  }, 60_000);

  it('refuses a release tag that disagrees with package.json', () => {
    const fixture = fixtureBundle();
    writeConfig(fixture.config, makeFixtureKey().pubkeyField);
    const result = run(['--bundle-dir', fixture.bundle, '--out-dir', join(fixture.dir, 'out'), '--config', fixture.config, '--repo', REPO, '--tag', 'v9.9.9']);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('version consistency');
  }, 60_000);
});

describe('post-upload verification (draft release)', () => {
  function staged() {
    const fixture = fixtureBundle();
    writeConfig(fixture.config, fixture.key.pubkeyField);
    const out = join(fixture.dir, 'out');
    const generated = run([
      '--bundle-dir', fixture.bundle,
      '--out-dir', out,
      '--config', fixture.config,
      '--repo', REPO,
      '--tag', TAG,
      '--pub-date', '2026-09-13T00:00:00Z',
    ]);
    expect(generated.status).toBe(0);
    const files = Object.fromEntries(
      parseChecksums(readFileSync(join(out, 'SHA256SUMS'), 'utf8')).map((row) => [row.name, readFileSync(join(out, row.name))]),
    );
    files.SHA256SUMS = readFileSync(join(out, 'SHA256SUMS'));
    return { fixture, out, files };
  }

  const args = (out: string, config: string) => ['--verify-remote', '--out-dir', out, '--config', config, '--repo', REPO, '--tag', TAG];

  it('accepts a draft whose uploaded bytes match SHA256SUMS', () => {
    const { fixture, out, files } = staged();
    const stub = stubGh(fixture.dir, { isDraft: true, assets: releaseAssets(files) }, files);
    const result = run(args(out, fixture.config), stub.env);
    expect(result.stderr).toBe('');
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('verified');
  }, 60_000);

  it('rejects an uploaded asset whose bytes were truncated', () => {
    const { fixture, out, files } = staged();
    const truncated = { ...files, [ARCHIVE]: files[ARCHIVE].subarray(0, 16) };
    const stub = stubGh(fixture.dir, { isDraft: true, assets: releaseAssets(truncated) }, truncated);
    const result = run(args(out, fixture.config), stub.env);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('sha256');
  }, 60_000);

  it('refuses to verify a release that is already published', () => {
    const { fixture, out, files } = staged();
    const stub = stubGh(fixture.dir, { isDraft: false, assets: releaseAssets(files) }, files);
    const result = run(args(out, fixture.config), stub.env);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('not a draft');
  }, 60_000);

  it('rejects an unexpected asset on the release', () => {
    const { fixture, out, files } = staged();
    const assets = [...releaseAssets(files), { name: 'notes.txt', url: `https://github.com/${REPO}/releases/download/${TAG}/notes.txt`, state: 'uploaded', size: 1 }];
    const stub = stubGh(fixture.dir, { isDraft: true, assets }, files);
    const result = run(args(out, fixture.config), stub.env);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('unexpected asset');
  }, 60_000);
});
