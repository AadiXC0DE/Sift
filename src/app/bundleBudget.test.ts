// Guards the eager-JS budget (P10.2). The gate is only meaningful if it is a
// build-time regression check, so package.json runs it after `vite build`; these
// cases pin the measurement rules the gate depends on.
import { describe, it, expect, afterAll } from 'vitest';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { DEFAULT_EAGER_JS_GATE_KIB, checkEagerBudget, measureEagerJs } from '../../scripts/eager-js.mjs';

const KIB = 1024;
const dirs: string[] = [];

function writeDist(files: Record<string, string | Buffer>, manifest?: unknown): string {
  const dir = mkdtempSync(join(tmpdir(), 'sift-eager-'));
  dirs.push(dir);
  for (const [rel, content] of Object.entries(files)) {
    const abs = join(dir, rel);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, content);
  }
  if (manifest) {
    mkdirSync(join(dir, '.vite'), { recursive: true });
    writeFileSync(join(dir, '.vite', 'manifest.json'), JSON.stringify(manifest));
  }
  return dir;
}

// Manifest mirroring Vite's: entry chunk plus its static imports.
const entryManifest = (deps: string[]) => ({
  'src/main.tsx': { file: 'assets/index.js', isEntry: true, imports: deps, dynamicImports: [] },
  ...Object.fromEntries(deps.map((d) => [d, { file: `assets/${d.replace(/[^a-z0-9]/gi, '_')}.js` }])),
});

afterAll(() => {
  for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
});

describe('eager JS budget', () => {
  it('gates at 250 KiB and fails when a static import pushes the payload over', () => {
    const dir = writeDist(
      {
        'index.html': '<div id="root"></div>',
        'assets/index.js': randomBytes(4 * KIB),
        'assets/_vendor_js.js': randomBytes(320 * KIB),
      },
      entryManifest(['_vendor.js']),
    );

    const result = checkEagerBudget(dir);
    expect(DEFAULT_EAGER_JS_GATE_KIB).toBe(250);
    expect(result.gateBytes).toBe(250 * KIB);
    expect(result.eagerBytes).toBeGreaterThan(result.gateBytes);
    expect(result.ok).toBe(false);
    expect(result.chunks.map((c) => c.file).sort()).toEqual(['assets/_vendor_js.js', 'assets/index.js']);
  });

  it('passes when the same large chunk is only reached through a dynamic import', () => {
    const dir = writeDist(
      {
        'index.html': '<div id="root"></div>',
        'assets/index.js': randomBytes(4 * KIB),
        'assets/heavy.js': randomBytes(320 * KIB),
      },
      {
        'src/main.tsx': {
          file: 'assets/index.js',
          isEntry: true,
          imports: [],
          dynamicImports: ['src/heavy.tsx'],
        },
        'src/heavy.tsx': { file: 'assets/heavy.js', imports: [] },
      },
    );

    const result = checkEagerBudget(dir);
    expect(result.ok).toBe(true);
    expect(result.chunks.map((c) => c.file)).toEqual(['assets/index.js']);
  });

  it('walks transitive static imports when no manifest was emitted', () => {
    const dir = writeDist({
      'index.html':
        '<script type="module" crossorigin src="/assets/index.js"></script>\n' +
        '<link rel="modulepreload" crossorigin href="/assets/preload.js">',
      'assets/index.js':
        'import{a}from"./nested.js";export{b}from"./preload.js";import("./lazy.js");console.log(a);',
      'assets/nested.js': 'export const a=1;',
      'assets/preload.js': 'export const b=2;',
      'assets/lazy.js': 'export const c=3;',
    });

    const result = measureEagerJs(dir);
    expect(result.source).toBe('index-html');
    expect(result.chunks.map((c) => c.file).sort()).toEqual([
      'assets/index.js',
      'assets/nested.js',
      'assets/preload.js',
    ]);
  });

  it('fails clearly instead of measuring nothing', () => {
    const dir = writeDist({ 'assets/orphan.js': 'x' });
    expect(() => measureEagerJs(dir)).toThrow(/no build found/);
  });
});
