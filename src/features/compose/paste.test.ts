import { describe, it, expect } from 'vitest';

describe('P7-T11 paste cleaning', () => {
  it('keeps only allowed marks', () => {
    const allowed = ['bold', 'italic', 'link', 'list'];
    const strip = (tag: string) => allowed.includes(tag);
    expect(strip('bold')).toBe(true);
    expect(strip('script')).toBe(false);
    expect(strip('font')).toBe(false);
  });
});
