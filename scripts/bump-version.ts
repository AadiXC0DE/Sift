// Keep package.json, Cargo.toml, tauri.conf.json versions equal.
import { readFileSync, writeFileSync } from 'node:fs';
const v = process.argv[2];
if (!v) {
  console.error('usage: bump-version <x.y.z>');
  process.exit(1);
}
const pkg = JSON.parse(readFileSync('package.json', 'utf8'));
pkg.version = v;
writeFileSync('package.json', JSON.stringify(pkg, null, 2) + '\n');
let cargo = readFileSync('src-tauri/Cargo.toml', 'utf8');
cargo = cargo.replace(/^version\s*=\s*".*"/m, `version = "${v}"`);
writeFileSync('src-tauri/Cargo.toml', cargo);
let conf = readFileSync('src-tauri/tauri.conf.json', 'utf8');
conf = conf.replace(/"version":\s*".*?"/, `"version": "${v}"`);
writeFileSync('src-tauri/tauri.conf.json', conf);
console.log(`bumped to ${v}`);
