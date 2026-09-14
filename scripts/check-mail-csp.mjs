import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../src/features/thread-view/mail-shim.js', import.meta.url), 'utf8');
const config = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
const hash = `'sha256-${createHash('sha256').update(source).digest('base64')}'`;
const directive = config.app.security.csp.split(';').find(part => part.trim().startsWith('script-src '));
if (!directive?.split(/\s+/).includes(hash)) {
  console.error(`Mail frame script changed. Update script-src in tauri.conf.json with ${hash}`);
  process.exit(1);
}
console.log('Mail frame script matches the release CSP hash.');
