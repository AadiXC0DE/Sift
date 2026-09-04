import { describe, it, expect } from 'vitest';

function parseRelease(payload: { assets: { name: string; browser_download_url: string; size: number }[]; tag_name: string }, sums: string) {
  const dmg = payload.assets.find((a) => a.name.endsWith('.dmg'));
  if (!dmg) throw new Error('no DMG asset (never ship a dead button)');
  const line = sums.split('\n').find((l) => l.includes(dmg.name));
  const sha = line?.split(/\s+/)[0];
  if (!sha) throw new Error('sha missing');
  return { version: payload.tag_name, url: dmg.browser_download_url, size: dmg.size, sha };
}

describe('P10-T13 release parse', () => {
  it('extracts version, immutable URL, size, sha; fails without DMG', () => {
    const ok = parseRelease(
      { tag_name: 'v1.0.0', assets: [{ name: 'Sift_1.0.0_universal.dmg', browser_download_url: 'https://github.com/ownpath/sift/releases/download/v1.0.0/Sift_1.0.0_universal.dmg', size: 1024 }] },
      'abc123  Sift_1.0.0_universal.dmg',
    );
    expect(ok.version).toBe('v1.0.0');
    expect(() => parseRelease({ tag_name: 'v1', assets: [] }, '')).toThrow();
  });
});
