import { describe, it, expect, vi } from 'vitest';

describe('P8-T05 debounce 40ms + restore', () => {
  it('debounces', async () => {
    const fn = vi.fn();
    let t: ReturnType<typeof setTimeout> | null = null;
    const run = (v: string) => {
      if (t) clearTimeout(t);
      t = setTimeout(() => fn(v), 40);
    };
    run('a');
    run('ad');
    run('ada');
    await new Promise((r) => setTimeout(r, 60));
    expect(fn).toHaveBeenCalledTimes(1);
  });
});
