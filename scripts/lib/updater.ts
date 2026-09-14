// Signed-update contract for the release tooling.
//
// This mirrors what the app actually does at runtime:
//   * tauri-plugin-updater 2.11 `verify_signature`: the `signature` field in
//     latest.json is base64 of a minisign signature box; `plugins.updater.pubkey`
//     is base64 of the minisign public key file contents. Algorithm byte pairs
//     are `Ed` (legacy, signature over the raw bytes) and `ED` (signature over
//     the BLAKE2b-512 digest), and a second signature covers
//     `<signature[64]><trusted comment>`.
//   * the static latest.json shape: `{ version, notes?, pub_date?, platforms }`
//     with `platforms[target] = { signature, url }`.
//   * rejection is fail-closed: any malformed metadata, non-newer version,
//     missing platform, unreachable endpoint, truncated/absent bytes or bad
//     signature leaves the current install untouched.
//
// Pure except for node:crypto, so vitest (and the dry-run harness) can drive it.
import { createHash, createPublicKey, verify as ed25519Verify } from 'node:crypto';
import { compareVersions, parseVersion } from './version-sources';

export { compareVersions, parseVersion };

// Ed25519 SPKI DER prefix for a raw 32-byte public key.
const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

export const PLATFORM_TARGETS = ['darwin-aarch64', 'darwin-x86_64', 'darwin-universal'] as const;
export type PlatformTarget = (typeof PLATFORM_TARGETS)[number];

export type ReleasePlatform = { signature: string; url: string };
export type ReleaseManifest = {
  version: string;
  notes?: string;
  pub_date?: string;
  platforms: Record<string, ReleasePlatform>;
};

export type RejectCode =
  | 'bad-metadata'
  | 'bad-version'
  | 'wrong-platform'
  | 'unreachable'
  | 'truncated-asset'
  | 'bad-signature';

export type UpdateDecision =
  | { install: false; code: RejectCode; message: string }
  | { install: true; code: 'ok'; message: string; version: string; url: string };

/** Placeholder text shipped in tauri.conf.json until the maintainer configures a key. */
export const PLACEHOLDER_MARKERS = ['replace with', 'placeholder', 'changeme', 'example key'];

const BASE64_RE = /^[A-Za-z0-9+/]+={0,2}$/;

export function decodeBase64Strict(value: string, what: string): Buffer {
  if (value.length === 0 || value.length % 4 !== 0 || !BASE64_RE.test(value)) {
    throw new Error(`${what} is not valid base64`);
  }
  const buf = Buffer.from(value, 'base64');
  if (buf.toString('base64') !== value) throw new Error(`${what} is not canonical base64`);
  return buf;
}

export type MinisignPublicKey = { algorithm: 'Ed' | 'ED'; keyId: string; key: Buffer; comment: string };

function algorithmOf(bytes: Buffer): 'Ed' | 'ED' {
  const pair = bytes.toString('latin1', 0, 2);
  if (pair !== 'Ed' && pair !== 'ED') throw new Error(`unsupported minisign algorithm ${JSON.stringify(pair)}`);
  return pair;
}

/** `plugins.updater.pubkey`: base64 of the minisign `.pub` file contents. */
export function decodePublicKeyField(field: string): MinisignPublicKey {
  const text = decodeBase64Strict(field.trim(), 'updater public key').toString('utf8');
  const lines = text.split('\n');
  if (lines.length < 2) throw new Error('updater public key has no key line');
  const payload = decodeBase64Strict(lines[1].trim(), 'updater public key body');
  if (payload.length !== 42) throw new Error(`updater public key body is ${payload.length} bytes, expected 42`);
  return {
    algorithm: algorithmOf(payload),
    keyId: payload.subarray(2, 10).toString('hex'),
    key: payload.subarray(10, 42),
    comment: lines[0],
  };
}

/** Human-readable reason the configured key cannot sign anything, or null when it looks usable. */
export function publicKeyProblem(field: string | undefined): string | null {
  if (!field || field.trim() === '') return 'updater public key is empty';
  let decodedText: string;
  try {
    decodedText = decodeBase64Strict(field.trim(), 'updater public key').toString('utf8');
  } catch (err) {
    return `updater public key is invalid: ${(err as Error).message}`;
  }
  for (const marker of PLACEHOLDER_MARKERS) {
    if (decodedText.toLowerCase().includes(marker)) return `updater public key is still the placeholder (${decodedText.trim()})`;
  }
  try {
    decodePublicKeyField(field);
  } catch (err) {
    return `updater public key is invalid: ${(err as Error).message}`;
  }
  return null;
}

export type MinisignSignature = {
  keyId: string;
  prehashed: boolean;
  signature: Buffer;
  trustedComment: string;
  globalSignature: Buffer;
};

/** Decode a minisign signature box (the text content of a `.sig` file). */
export function decodeSignatureBox(text: string): MinisignSignature {
  const lines = text.split('\n');
  if (lines.length < 4) throw new Error('signature box has fewer than 4 lines');
  const payload = decodeBase64Strict(lines[1].trim(), 'signature body');
  if (payload.length !== 74) throw new Error(`signature body is ${payload.length} bytes, expected 74`);
  if (!lines[2].startsWith('trusted comment: ')) throw new Error('signature box has no trusted comment line');
  const global = decodeBase64Strict(lines[3].trim(), 'signature global body');
  if (global.length !== 64) throw new Error(`signature global body is ${global.length} bytes, expected 64`);
  return {
    keyId: payload.subarray(2, 10).toString('hex'),
    prehashed: algorithmOf(payload) === 'ED',
    signature: payload.subarray(10, 74),
    trustedComment: lines[2].slice('trusted comment: '.length),
    globalSignature: global,
  };
}

/** latest.json carries base64 of the minisign box, as Tauri writes in its `.sig` file. */
export function signatureFieldFromBox(boxText: string): string {
  return Buffer.from(boxText, 'utf8').toString('base64');
}

function ed25519VerifyRaw(message: Buffer, key: Buffer, signature: Buffer): boolean {
  const publicKey = createPublicKey({ key: Buffer.concat([ED25519_SPKI_PREFIX, key]), format: 'der', type: 'spki' });
  return ed25519Verify(null, message, publicKey, signature);
}

export type SignatureCheck = { ok: true } | { ok: false; reason: string };

/** Verify `data` against the base64 signature field using the configured public key field. */
export function verifyArtifactSignature(pubkeyField: string, signatureField: string, data: Buffer): SignatureCheck {
  let pubkey: MinisignPublicKey;
  let signature: MinisignSignature;
  try {
    pubkey = decodePublicKeyField(pubkeyField);
  } catch (err) {
    return { ok: false, reason: `public key unusable: ${(err as Error).message}` };
  }
  let boxText: string;
  try {
    boxText = decodeBase64Strict(signatureField, 'signature field').toString('utf8');
  } catch (err) {
    return { ok: false, reason: `signature field invalid: ${(err as Error).message}` };
  }
  try {
    signature = decodeSignatureBox(boxText);
  } catch (err) {
    return { ok: false, reason: `signature box invalid: ${(err as Error).message}` };
  }
  if (signature.keyId !== pubkey.keyId) {
    return { ok: false, reason: `signature key id ${signature.keyId} does not match public key ${pubkey.keyId}` };
  }
  const signed = signature.prehashed ? createHash('blake2b512').update(data).digest() : data;
  if (!ed25519VerifyRaw(signed, pubkey.key, signature.signature)) {
    return { ok: false, reason: 'artifact signature does not match artifact bytes' };
  }
  const global = Buffer.concat([signature.signature, Buffer.from(signature.trustedComment, 'utf8')]);
  if (!ed25519VerifyRaw(global, pubkey.key, signature.globalSignature)) {
    return { ok: false, reason: 'trusted-comment signature does not verify' };
  }
  return { ok: true };
}

export type ManifestParseOptions = {
  /**
   * Test seam for the staged fixture harness and unit tests: permits plain
   * http:// urls, but only for loopback hosts. Never set on the release path.
   */
  allowInsecureLoopback?: boolean;
};

function urlProblemInManifest(url: string, allowInsecureLoopback: boolean): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol === 'https:') return null;
    if (allowInsecureLoopback && parsed.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(parsed.hostname)) {
      return null;
    }
    return `not an https url (${url})`;
  } catch {
    return `not a url (${url})`;
  }
}

export function parseManifest(value: unknown, options: ManifestParseOptions = {}): ReleaseManifest {
  const allowInsecureLoopback = options.allowInsecureLoopback === true;
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error('latest.json is not an object');
  const raw = value as Record<string, unknown>;
  const version = typeof raw.version === 'string' ? raw.version.trim() : '';
  parseVersion(version);
  if (typeof raw.pub_date !== 'undefined' && typeof raw.pub_date !== 'string') {
    throw new Error('pub_date is not a string');
  }
  if (typeof raw.notes !== 'undefined' && typeof raw.notes !== 'string') throw new Error('notes is not a string');
  const platforms = raw.platforms;
  if (typeof platforms !== 'object' || platforms === null || Array.isArray(platforms)) {
    throw new Error('platforms is not an object');
  }
  const entries = Object.entries(platforms as Record<string, unknown>);
  if (entries.length === 0) throw new Error('platforms is empty');
  const parsed: Record<string, ReleasePlatform> = {};
  for (const [target, platform] of entries) {
    if (typeof platform !== 'object' || platform === null) throw new Error(`platforms.${target} is not an object`);
    const { signature, url } = platform as Record<string, unknown>;
    if (typeof signature !== 'string' || signature === '') throw new Error(`platforms.${target}.signature is missing`);
    if (typeof url !== 'string') throw new Error(`platforms.${target}.url is missing`);
    const problem = urlProblemInManifest(url, allowInsecureLoopback);
    if (problem) throw new Error(`platforms.${target}.url ${problem}`);
    parsed[target] = { signature, url };
  }
  return {
    version,
    notes: typeof raw.notes === 'string' ? raw.notes : undefined,
    pub_date: typeof raw.pub_date === 'string' ? raw.pub_date : undefined,
    platforms: parsed,
  };
}

/** tauri-plugin-updater resolves `darwin-<arch>` first, then `darwin-universal`. */
export function selectPlatform(manifest: ReleaseManifest, target: string): ReleasePlatform | null {
  const direct = manifest.platforms[target];
  if (direct) return direct;
  if (target.startsWith('darwin-')) return manifest.platforms['darwin-universal'] ?? null;
  return null;
}

export type UpdateInput = {
  manifest: unknown;
  currentVersion: string;
  target: string;
  pubkey: string;
  /** only the staged fixture harness sets this; see ManifestParseOptions. */
  allowInsecureLoopback?: boolean;
  /** null models an unreachable endpoint; `expectedSha256` models a published SHA256SUMS row. */
  download?: { bytes: Buffer; expectedSha256?: string } | null;
};

export function evaluateUpdate(input: UpdateInput): UpdateDecision {
  let manifest: ReleaseManifest;
  try {
    manifest = parseManifest(input.manifest, { allowInsecureLoopback: input.allowInsecureLoopback });
  } catch (err) {
    return { install: false, code: 'bad-metadata', message: `update metadata rejected: ${(err as Error).message}` };
  }
  if (compareVersions(manifest.version, input.currentVersion) <= 0) {
    return {
      install: false,
      code: 'bad-version',
      message: `update version ${manifest.version} is not newer than ${input.currentVersion}`,
    };
  }
  const platform = selectPlatform(manifest, input.target);
  if (!platform) {
    return {
      install: false,
      code: 'wrong-platform',
      message: `update has no artifact for ${input.target} (has ${Object.keys(manifest.platforms).join(', ')})`,
    };
  }
  if (!input.download) {
    return { install: false, code: 'unreachable', message: `could not download ${platform.url}` };
  }
  const bytes = input.download.bytes;
  if (input.download.expectedSha256) {
    const actual = createHash('sha256').update(bytes).digest('hex');
    if (actual !== input.download.expectedSha256) {
      return {
        install: false,
        code: 'truncated-asset',
        message: `downloaded asset sha256 ${actual} does not match published ${input.download.expectedSha256}`,
      };
    }
  }
  const check = verifyArtifactSignature(input.pubkey, platform.signature, bytes);
  if (!check.ok) {
    return { install: false, code: 'bad-signature', message: `signature rejected: ${check.reason}` };
  }
  return {
    install: true,
    code: 'ok',
    message: `verified ${manifest.version} for ${input.target}`,
    version: manifest.version,
    url: platform.url,
  };
}
