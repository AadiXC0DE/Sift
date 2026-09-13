// Test/fixture-only minisign signing helpers.
//
// Used by scripts/updater-dryrun.ts and scripts/lib/updater.test.ts to build
// synthetic signed artifacts. Keys are generated in memory per call; this file
// is never part of the app or of a real release.
import { createHash, generateKeyPairSync, randomBytes, sign, type KeyObject } from 'node:crypto';

export type FixtureKey = { privateKey: KeyObject; keyId: Buffer; pubkeyField: string };

export function makeFixtureKey(): FixtureKey {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(12);
  const keyId = randomBytes(8);
  const file = `untrusted comment: minisign public key ${keyId.toString('hex').toUpperCase()}\n${Buffer.concat([
    Buffer.from('ED', 'utf8'),
    keyId,
    raw,
  ]).toString('base64')}\n`;
  return { privateKey, keyId, pubkeyField: Buffer.from(file, 'utf8').toString('base64') };
}

/** Produce a minisign signature box the same shape tauri's signer emits. */
export function signBox(key: FixtureKey, data: Buffer, fileName: string, prehashed = true): string {
  const digest = prehashed ? createHash('blake2b512').update(data).digest() : data;
  const signature = sign(null, digest, key.privateKey);
  const trustedComment = `timestamp:${Math.floor(Date.now() / 1000)}\tfile:${fileName}${prehashed ? '\tprehashed' : ''}`;
  const global = sign(null, Buffer.concat([signature, Buffer.from(trustedComment, 'utf8')]), key.privateKey);
  const payload = Buffer.concat([Buffer.from(prehashed ? 'ED' : 'Ed', 'utf8'), key.keyId, signature]);
  return `untrusted comment: signature from tauri secret key\n${payload.toString('base64')}\ntrusted comment: ${trustedComment}\n${global.toString('base64')}\n`;
}

/** latest.json carries base64 of the whole box, matching tauri-plugin-updater. */
export function signatureField(box: string): string {
  return Buffer.from(box, 'utf8').toString('base64');
}

/** Flip a bit in the signature so the box is well-formed but wrong. */
export function corruptBox(box: string): string {
  const lines = box.split('\n');
  const payload = Buffer.from(lines[1], 'base64');
  payload[payload.length - 1] ^= 0xff;
  lines[1] = payload.toString('base64');
  return lines.join('\n');
}
