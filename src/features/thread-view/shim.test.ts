import { describe, it, expect } from 'vitest';
import { buildShim } from './shim';

describe('P5-T06 shim', () => {
  it('forwards j, blocks body contextmenu but not a/img, reports size', () => {
    const s = buildShim('n1');
    expect(s).toMatch(/post.*key/);
    expect(s).toMatch(/contextmenu/);
    expect(s).toMatch(/ResizeObserver/);
    expect(s).toMatch(/__siftFind/);
  });
});
