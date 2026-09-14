// Configure the updater public key in src-tauri/tauri.conf.json.
//
//   pnpm exec tsx scripts/set-updater-pubkey.ts ~/.tauri/sift.key.pub
//
// The key pair is generated with `pnpm tauri signer generate -w ~/.tauri/sift.key`.
// This script only ever reads the *public* half: it refuses anything that looks
// like a secret key, so the private key can never end up in source or logs.
import { readFileSync, writeFileSync } from 'node:fs';
import { DEFAULT_CONFIG, requireUsablePublicKey } from './lib/release-artifacts';
import { decodePublicKeyField, publicKeyProblem } from './lib/updater';

const args = process.argv.slice(2);
const configFlag = args.indexOf('--config');
const configPath = configFlag === -1 ? DEFAULT_CONFIG : args[configFlag + 1];
const positional = args.filter((arg, index) => !arg.startsWith('--') && (configFlag === -1 || index !== configFlag + 1));
const keyPath = positional[0];
if (!keyPath) {
  console.error('usage: set-updater-pubkey <path-to-minisign-public-key>');
  process.exit(1);
}
const fileContent = readFileSync(keyPath, 'utf8').trim();
// Tauri writes base64-encoded minisign files; also accept plain minisign keys.
const content = fileContent.startsWith('untrusted comment:')
  ? fileContent
  : Buffer.from(fileContent, 'base64').toString('utf8').trim();
if (/secret key/i.test(content) || /PRIVATE KEY/.test(content)) {
  console.error(`${keyPath} looks like a *private* key; pass the .pub file instead`);
  process.exit(1);
}
if (!content.includes('minisign public key')) {
  console.error(`${keyPath} is not a minisign public key file (expected an "untrusted comment: minisign public key ..." line)`);
  process.exit(1);
}
const field = Buffer.from(content, 'utf8').toString('base64');
const problem = publicKeyProblem(field);
if (problem) {
  console.error(`${keyPath}: ${problem}`);
  process.exit(1);
}
const key = decodePublicKeyField(field);

const raw = readFileSync(configPath, 'utf8');
if (!/"pubkey":\s*"[^"]*"/.test(raw)) {
  console.error(`${configPath}: no plugins.updater.pubkey field to replace`);
  process.exit(1);
}
writeFileSync(configPath, raw.replace(/"pubkey":\s*"[^"]*"/, `"pubkey": "${field}"`));
console.log(`wrote ${configPath} (key id ${key.keyId}, comment: ${key.comment})`);
console.log('next: store the private key and its password as TAURI_SIGNING_PRIVATE_KEY / TAURI_SIGNING_PRIVATE_KEY_PASSWORD');
// Re-read the written config so a broken edit fails here instead of in CI.
requireUsablePublicKey(configPath);
