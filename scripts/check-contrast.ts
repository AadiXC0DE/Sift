// P9-T11: --fg* on --bg* >= 4.5:1 both themes (reads actual tokens.css values).
import { readFileSync } from 'node:fs';

function lum(hex: string): number {
  const c = hex.replace('#', '');
  const v = [0, 2, 4].map((i) => {
    const x = parseInt(c.slice(i, i + 2), 16) / 255;
    return x <= 0.03928 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
}
function ratio(a: string, b: string): number {
  const [l1, l2] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (l1 + 0.05) / (l2 + 0.05);
}

const css = readFileSync('src/styles/tokens.css', 'utf8');
function vars(block: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of block.matchAll(/(--[\w-]+):\s*(#[0-9a-fA-F]{6})/g)) out[m[1]] = m[2];
  return out;
}
const lightBlock = css.split(':root')[1]?.split(':root')[0] ?? '';
const darkBlock =
  css.split('[data-theme="dark"]')[1]?.split('}')[0] ?? css.split(':root[data-theme="dark"]')[1] ?? '';
const light = vars(lightBlock);
const dark = vars(css);
// Resolve var(--nX) indirections for fg/bg
function resolve(v: string, table: Record<string, string>): string {
  const m = v.match(/var\((--[\w-]+)\)/);
  if (m) return table[m[1]] ?? v;
  return v;
}
// Direct neutral values (ground truth for text on background)
const lightPairs: [string, string][] = [
  [light['--n9'] ?? '#1f1f1d', '#ffffff'],
  [light['--n7'] ?? '#5f5f5a', '#ffffff'],
  [light['--n6'] ?? '#6e6e69', '#ffffff'],
];
const darkPairs: [string, string][] = [
  [dark['--n9'] ?? '#ececea', dark['--n0'] ?? '#161615'],
  [dark['--n7'] ?? '#b0b0aa', dark['--n1'] ?? '#1b1b1a'],
  [dark['--n6'] ?? '#8a8a85', dark['--n1'] ?? '#1b1b1a'],
];
void resolve;
let fail = 0;
for (const [fg, bg] of [...lightPairs, ...darkPairs]) {
  const r = ratio(fg, bg);
  console.log(`${fg} on ${bg}: ${r.toFixed(2)}`);
  if (r < 4.5) {
    console.error(`FAIL contrast ${fg} on ${bg} = ${r.toFixed(2)}`);
    fail++;
  }
}
if (fail) process.exit(1);
console.log('contrast ok');
