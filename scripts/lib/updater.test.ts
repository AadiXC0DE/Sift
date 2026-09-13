// Contract tests for the signed-update path: what the release pipeline
// generates must be exactly what the updater verifies, and every failure mode
// must leave the current install alone.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { buildChecksums, parseChecksums, secretMaterialProblem, urlProblem } from './release-artifacts';
import { corruptBox, makeFixtureKey, signBox, signatureField } from './fixture-signing';
import {
  compareVersions,
  decodePublicKeyField,
  decodeSignatureBox,
  evaluateUpdate,
  publicKeyProblem,
  verifyArtifactSignature,
  type UpdateDecision,
} from './updater';

const key = makeFixtureKey();
const otherKey = makeFixtureKey();
const archive = Buffer.from('signed updater archive bytes');
const box = signBox(key, archive, 'Sift.app.tar.gz');
const signature = signatureField(box);
const pubkey = key.pubkeyField;

const base = { currentVersion: '1.0.0', target: 'darwin-aarch64', pubkey };
const ARCHIVE_URL = 'https://github.com/AadiXC0DE/Sift/releases/download/v1.0.1/Sift.app.tar.gz';

function manifest(overrides: Record<string, unknown> = {}) {
  return {
    version: '1.0.1',
    pub_date: '2026-09-13T00:00:00Z',
    platforms: { 'darwin-aarch64': { signature, url: ARCHIVE_URL } },
    ...overrides,
  };
}

function decide(overrides: Partial<Parameters<typeof evaluateUpdate>[0]> = {}): UpdateDecision {
  return evaluateUpdate({ ...base, manifest: manifest(), download: { bytes: archive }, ...overrides });
}

describe('signed update decision', () => {
  it('installs a correctly signed newer release', () => {
    expect(decide()).toMatchObject({ install: true, code: 'ok', version: '1.0.1' });
  });

  it('verifies both minisign algorithms tauri accepts', () => {
    const legacy = signatureField(signBox(key, archive, 'Sift.app.tar.gz', false));
    const legacyManifest = manifest({ platforms: { 'darwin-aarch64': { signature: legacy, url: ARCHIVE_URL } } });
    expect(evaluateUpdate({ ...base, manifest: legacyManifest, download: { bytes: archive } })).toMatchObject({ install: true });
    expect(decodeSignatureBox(box).prehashed).toBe(true);
    expect(decodeSignatureBox(signBox(key, archive, 'Sift.app.tar.gz', false)).prehashed).toBe(false);
  });

  it('keeps the current install on a bad signature', () => {
    const bad = manifest({ platforms: { 'darwin-aarch64': { signature: signatureField(corruptBox(box)), url: ARCHIVE_URL } } });
    expect(evaluateUpdate({ ...base, manifest: bad, download: { bytes: archive } })).toMatchObject({
      install: false,
      code: 'bad-signature',
    });
  });

  it('keeps the current install when the artifact does not match the signature', () => {
    const tampered = Buffer.from(archive);
    tampered[0] ^= 0xff;
    expect(decide({ download: { bytes: tampered } })).toMatchObject({ install: false, code: 'bad-signature' });
  });

  it('keeps the current install when the signature is from another key', () => {
    expect(decide({ pubkey: otherKey.pubkeyField })).toMatchObject({ install: false, code: 'bad-signature' });
    expect(verifyArtifactSignature(otherKey.pubkeyField, signature, archive)).toMatchObject({ ok: false });
  });

  it('rejects a truncated asset against published checksums and by signature', () => {
    const truncated = { bytes: archive.subarray(0, 8), expectedSha256: 'ff'.repeat(32) };
    expect(decide({ download: truncated })).toMatchObject({ install: false, code: 'truncated-asset' });
    expect(decide({ download: { bytes: archive.subarray(0, 8) } })).toMatchObject({ install: false, code: 'bad-signature' });
  });

  it('refuses an equal, older or unparseable version', () => {
    expect(decide({ manifest: manifest({ version: '1.0.0' }) })).toMatchObject({ install: false, code: 'bad-version' });
    expect(decide({ manifest: manifest({ version: '0.9.9' }) })).toMatchObject({ install: false, code: 'bad-version' });
    expect(decide({ manifest: manifest({ version: 'latest' }) })).toMatchObject({ install: false, code: 'bad-metadata' });
    expect(compareVersions('1.10.0', '1.9.9')).toBe(1);
  });

  it('refuses an update that has no artifact for this platform, but falls back to universal', () => {
    const onlyWindows = manifest({ platforms: { 'win32-x64': { signature, url: ARCHIVE_URL } } });
    expect(evaluateUpdate({ ...base, manifest: onlyWindows, download: { bytes: archive } })).toMatchObject({
      install: false,
      code: 'wrong-platform',
    });
    const universal = manifest({
      platforms: { 'darwin-universal': { signature, url: 'https://github.com/AadiXC0DE/Sift/releases/download/v1.0.1/Sift.app.tar.gz' } },
    });
    expect(evaluateUpdate({ ...base, target: 'darwin-x86_64', manifest: universal, download: { bytes: archive } })).toMatchObject({
      install: true,
    });
  });

  it('keeps the current install when the endpoint or asset is unreachable', () => {
    expect(decide({ download: null })).toMatchObject({ install: false, code: 'unreachable' });
  });

  it('rejects malformed metadata instead of guessing', () => {
    expect(evaluateUpdate({ ...base, manifest: 'not json', download: { bytes: archive } })).toMatchObject({ code: 'bad-metadata' });
    expect(decide({ manifest: { version: '1.0.1' } })).toMatchObject({ code: 'bad-metadata' });
    expect(decide({ manifest: manifest({ platforms: {} }) })).toMatchObject({ code: 'bad-metadata' });
    expect(decide({ manifest: manifest({ platforms: { 'darwin-aarch64': { url: ARCHIVE_URL } } }) })).toMatchObject({ code: 'bad-metadata' });
    expect(decide({ manifest: manifest({ platforms: { 'darwin-aarch64': { signature, url: 'http://evil.example/x' } } }) })).toMatchObject({
      code: 'bad-metadata',
    });
  });
});

describe('updater public key gate', () => {
  it('rejects the checked-in config until a real key is configured', () => {
    const config: unknown = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
    const configured = (config as { plugins?: { updater?: { pubkey?: string } } }).plugins?.updater?.pubkey ?? '';
    const problem = publicKeyProblem(configured);
    if (problem) {
      // The pipeline must fail loudly rather than ship an unverifiable update path.
      expect(problem).toMatch(/placeholder|invalid|empty/);
      expect(evaluateUpdate({ ...base, pubkey: configured, manifest: manifest(), download: { bytes: archive } })).toMatchObject({
        install: false,
        code: 'bad-signature',
      });
    } else {
      expect(() => decodePublicKeyField(configured)).not.toThrow();
    }
  });

  it('accepts a well-formed minisign key and rejects broken ones', () => {
    expect(publicKeyProblem(pubkey)).toBeNull();
    expect(decodePublicKeyField(pubkey).keyId).toBe(key.keyId.toString('hex'));
    expect(publicKeyProblem(Buffer.from('untrusted comment: replace with real updater public key').toString('base64'))).toContain('placeholder');
    expect(publicKeyProblem('not base64!!')).toContain('invalid');
    expect(publicKeyProblem('')).toContain('empty');
  });
});

describe('release metadata', () => {
  it('round-trips checksum rows and rejects junk', () => {
    const text = buildChecksums([
      { name: 'b.dmg', sha256: 'bb'.repeat(32) },
      { name: 'a.tar.gz', sha256: 'aa'.repeat(32) },
    ]);
    expect(text.startsWith('aa'.repeat(32))).toBe(true);
    expect(parseChecksums(text)).toHaveLength(2);
    expect(() => parseChecksums('not a checksum line')).toThrow();
  });

  it('rejects platform urls that are not uploaded assets', () => {
    const expected = { repo: 'AadiXC0DE/Sift', tag: 'v1.0.1', name: 'Sift.app.tar.gz' };
    expect(urlProblem('https://github.com/AadiXC0DE/Sift/releases/download/v1.0.1/Sift.app.tar.gz', expected)).toBeNull();
    expect(urlProblem('https://github.com/AadiXC0DE/Sift/archive/refs/tags/v1.0.1.tar.gz', expected)).toContain('source archive');
    expect(urlProblem('https://github.com/Other/Sift/releases/download/v1.0.1/Sift.app.tar.gz', expected)).toContain('must live under');
    expect(urlProblem('https://github.com/AadiXC0DE/Sift/releases/download/v2.0.0/Sift.app.tar.gz', expected)).toContain('must live under');
    expect(urlProblem('http://github.com/AadiXC0DE/Sift/releases/download/v1.0.1/Sift.app.tar.gz', expected)).toContain('https');
  });

  it('refuses to emit private-key material but allows tauri signature boxes', () => {
    expect(secretMaterialProblem('untrusted comment: minisign encrypted secret key')).toContain('minisign secret key');
    expect(secretMaterialProblem('-----BEGIN PRIVATE KEY-----')).toContain('PEM private key');
    expect(secretMaterialProblem('untrusted comment: signature from tauri secret key')).toBeNull();
    expect(secretMaterialProblem(JSON.stringify({ version: '1.0.0' }))).toBeNull();
  });

  it('rejects a signature box whose payload is the wrong length', () => {
    const lines = box.split('\n');
    lines[1] = Buffer.from('too short', 'utf8').toString('base64');
    expect(() => decodeSignatureBox(lines.join('\n'))).toThrow(/expected 74/);
  });
});
