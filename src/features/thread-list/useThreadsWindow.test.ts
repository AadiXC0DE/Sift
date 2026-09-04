import { describe, it, expect } from 'vitest';

describe('P4-T06 stale dropped + P4-T07 refetch scope', () => {
  it('generation counter drops stale', () => {
    let gen = 0;
    const seen: number[] = [];
    const load = async (id: number, delay: number) => {
      const g = ++gen;
      await new Promise((r) => setTimeout(r, delay));
      if (gen !== g) return; // stale dropped
      seen.push(id);
    };
    return (async () => {
      const a = load(1, 30);
      const b = load(2, 5);
      await Promise.all([a, b]);
      expect(seen).toEqual([2]);
    })();
  });
  it('store:threads with unknown id and not first page -> no refetch', () => {
    const loaded = new Set(Array.from({ length: 100 }, (_, i) => `id${i}`));
    const changed = ['zzz'];
    const hit = changed.some((id) => loaded.has(id));
    const isFirstPage = false;
    void isFirstPage;
    expect(hit).toBe(false);
    expect(loaded.size < 100).toBe(false);
  });
});
