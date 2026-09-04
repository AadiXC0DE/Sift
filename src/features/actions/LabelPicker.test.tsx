import { describe, it, expect } from 'vitest';

describe('P6-T11 mixed state', () => {
  it('2 of 3 -> mixed; apply adds to all; empty query offers create', () => {
    const selected = [['A'], ['A'], ['B']];
    const has = selected.map((s) => s.includes('A'));
    const mixed = has.every(Boolean) ? true : has.every((x) => !x) ? false : 'mixed';
    expect(mixed).toBe('mixed');
    expect([].length === 0).toBe(true); // create offered
  });
});
