#!/usr/bin/env node
// Measures the eager JavaScript payload of a Vite build and enforces the
// small-app budget. "Eager" means every chunk the browser must fetch and parse
// before the shell can run: the HTML entry chunk plus its transitive static
// imports. Dynamic imports (React.lazy, prefetch, route splits) are excluded
// by construction — that is the whole point of the budget.
//
// Usage:
//   node scripts/eager-js.mjs [--dist <dir>] [--max-js-kib <n>] [--json]
// Exit codes: 0 = within budget, 1 = over budget, 2 = cannot measure.
import { existsSync, readFileSync, statSync } from 'node:fs';
import { join, posix, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

export const KIB = 1024;
export const DEFAULT_EAGER_JS_GATE_KIB = 250;

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

function gzipSize(abs) {
  // level 9 with no filename/timestamp in the header: deterministic across runs.
  return gzipSync(readFileSync(abs), { level: 9 }).length;
}

function toDistPath(distDir, urlPath) {
  return join(distDir, urlPath.replace(/^\/+/, ''));
}

function makeChunk(distDir, key, file) {
  const abs = join(distDir, file);
  if (!existsSync(abs)) {
    throw new Error(`manifest references a missing file: ${file}`);
  }
  const rawBytes = statSync(abs).size;
  return { key, file, rawBytes, gzipBytes: gzipSize(abs) };
}

/** Hrefs of `<script type="module" src>` and `<link rel="modulepreload" href>`. */
function htmlEagerFiles(distDir) {
  const html = readFileSync(join(distDir, 'index.html'), 'utf8');
  const files = [];
  for (const tag of html.match(/<script\b[^>]*>/g) ?? []) {
    if (!/type="module"/.test(tag)) continue;
    const src = /\bsrc="([^"]+)"/.exec(tag);
    if (src) files.push(src[1]);
  }
  for (const tag of html.match(/<link\b[^>]*>/g) ?? []) {
    if (!/rel="modulepreload"/.test(tag)) continue;
    const href = /\bhref="([^"]+)"/.exec(tag);
    if (href) files.push(href[1]);
  }
  return files;
}

/** Static (not dynamic) import specifiers of a built chunk, as relative files. */
function staticImports(absFile) {
  const code = readFileSync(absFile, 'utf8');
  const out = new Set();
  const re = /(?:^|[^\w$.])import\s*(?:[^"'`()]*?from\s*)?["'](\.\/[^"']+)["']/g;
  for (let m = re.exec(code); m; m = re.exec(code)) out.add(m[1]);
  const reExport = /(?:^|[^\w$.])export[^"'`()]*?from\s*["'](\.\/[^"']+)["']/g;
  for (let m = reExport.exec(code); m; m = reExport.exec(code)) out.add(m[1]);
  return [...out];
}

/**
 * Walks dist/.vite/manifest.json from every entry through `imports` (static
 * only, never `dynamicImports`). Falls back to index.html + a static-import
 * closure when the manifest was not emitted.
 */
export function measureEagerJs(distDir) {
  const root = resolve(distDir);
  if (!existsSync(join(root, 'index.html'))) {
    throw new Error(`no build found at ${root} (missing index.html)`);
  }
  const manifestPath = join(root, '.vite', 'manifest.json');
  const chunks = [];
  let entryKeys = [];

  if (existsSync(manifestPath)) {
    const manifest = readJson(manifestPath);
    entryKeys = Object.keys(manifest).filter((k) => manifest[k].isEntry);
    if (entryKeys.length === 0) {
      throw new Error(`${manifestPath} has no isEntry chunk`);
    }
    const seen = new Set();
    const queue = [...entryKeys];
    while (queue.length > 0) {
      const key = queue.shift();
      if (seen.has(key)) continue;
      seen.add(key);
      const chunk = manifest[key];
      if (!chunk) throw new Error(`manifest entry ${key} is missing`);
      chunks.push(makeChunk(root, key, chunk.file));
      for (const dep of chunk.imports ?? []) queue.push(dep);
    }
  } else {
    const entryFiles = htmlEagerFiles(root);
    if (entryFiles.length === 0) {
      throw new Error(`${root}/index.html lists no module script or modulepreload`);
    }
    entryKeys = entryFiles;
    const seen = new Set();
    const queue = entryFiles.map((f) => f.replace(/^\/+/, ''));
    while (queue.length > 0) {
      const rel = queue.shift();
      if (seen.has(rel)) continue;
      seen.add(rel);
      chunks.push(makeChunk(root, rel, rel));
      for (const spec of staticImports(join(root, rel))) {
        queue.push(posix.join(posix.dirname(rel), spec));
      }
    }
  }

  chunks.sort((a, b) => b.gzipBytes - a.gzipBytes);
  return {
    source: existsSync(manifestPath) ? 'manifest' : 'index-html',
    distDir: root,
    entryKeys,
    chunks,
    rawBytes: chunks.reduce((n, c) => n + c.rawBytes, 0),
    eagerBytes: chunks.reduce((n, c) => n + c.gzipBytes, 0),
  };
}

/** Measured payload plus the gate verdict. */
export function checkEagerBudget(distDir, gateKib = DEFAULT_EAGER_JS_GATE_KIB) {
  const measured = measureEagerJs(distDir);
  const gateBytes = gateKib * KIB;
  return { ...measured, gateKib, gateBytes, ok: measured.eagerBytes <= gateBytes };
}

export function formatReport(result) {
  const lines = [`eager JS (${result.source}, ${result.distDir})`];
  for (const c of result.chunks) {
    lines.push(`  ${c.file}  raw=${c.rawBytes}  gzip=${c.gzipBytes}`);
  }
  lines.push(
    `  total eager gzip=${result.eagerBytes} B (${(result.eagerBytes / KIB).toFixed(1)} KiB) ` +
      `raw=${result.rawBytes} B`,
  );
  lines.push(`  gate=${result.gateKib} KiB (${result.gateBytes} B) -> ${result.ok ? 'PASS' : 'FAIL'}`);
  return lines.join('\n');
}

function parseArgs(argv) {
  const opts = { dist: 'dist', maxJsKib: DEFAULT_EAGER_JS_GATE_KIB, json: false };
  const value = (i) => argv[i + 1];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const eq = arg.indexOf('=');
    const [flag, inline] = eq > 0 ? [arg.slice(0, eq), arg.slice(eq + 1)] : [arg, undefined];
    if (flag === '--dist') opts.dist = inline ?? value(i++);
    else if (flag === '--max-js-kib') opts.maxJsKib = Number(inline ?? value(i++));
    else if (flag === '--json') opts.json = true;
    else throw new Error(`unknown argument: ${arg}`);
  }
  if (!Number.isFinite(opts.maxJsKib) || opts.maxJsKib <= 0) {
    throw new Error(`invalid --max-js-kib: ${opts.maxJsKib}`);
  }
  return opts;
}

const invokedDirectly = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  try {
    const opts = parseArgs(process.argv.slice(2));
    const result = checkEagerBudget(opts.dist, opts.maxJsKib);
    if (opts.json) console.log(JSON.stringify(result, null, 2));
    else console.log(formatReport(result));
    process.exit(result.ok ? 0 : 1);
  } catch (err) {
    console.error(`eager-js: ${err instanceof Error ? err.message : String(err)}`);
    process.exit(2);
  }
}
