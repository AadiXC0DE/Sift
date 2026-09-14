import { describe, it, expect } from 'vitest';
import { buildShim } from './shim';

describe('P5-T06 shim', () => {
  it('forwards j, blocks body contextmenu but not a/img, reports size', () => {
    const s = buildShim('n1', 'tok-123');
    expect(s).toMatch(/post.*key/);
    expect(s).toMatch(/contextmenu/);
    expect(s).toMatch(/ResizeObserver/);
    expect(s).toMatch(/__siftFind/);
  });

  it('tags every message with the frame token (P9.2)', () => {
    const s = buildShim('n1', 'tok-123');
    expect(s).toContain('var TOKEN = "tok-123"');
    expect(s).toMatch(/postMessage\(\{ __sift: true, token: TOKEN/);
  });

  it('forwards only unmodified navigation keys (P9.2)', () => {
    const s = buildShim('n1', 'tok-123');
    // Modifier chords and IME composition never leave the frame, so mail HTML
    // cannot reach an application command through the key relay.
    expect(s).toMatch(/if \(e\.metaKey \|\| e\.ctrlKey \|\| e\.altKey\) return;/);
    expect(s).toMatch(/if \(e\.isComposing \|\| e\.keyCode === 229\) return;/);
  });
});
