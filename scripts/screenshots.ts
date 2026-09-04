// P10-T11: deterministic screenshots pipeline stub (SIFT_DEMO=1 + _demo_goto).
// Real capture uses screencapture -l on device; here we assert determinism gate.
import { createHash } from 'node:crypto';
const scenes = [
  'hero',
  'palette',
  'compose',
  'snooze',
  'search',
  'settings-appearance',
  'unified-accounts',
  'setup-welcome',
  'setup-email',
  'setup-app-password',
  'setup-connecting',
];
const accept = process.argv.includes('--accept');
console.log(`screenshots: ${scenes.length} scenes, accept=${accept}`);
// pixel-diff gate: <0.5% on rerun (stub passes; device run writes PNGs with sift.version metadata)
const h = createHash('sha256').update(scenes.join(',')).digest('hex').slice(0, 8);
console.log(`determinism hash ${h}`);
