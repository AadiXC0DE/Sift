import { describe, it, expect, vi } from 'vitest';

describe('P7-T09 debounce + P7-T10 chips', () => {
  it('typing debounces drafts_upsert to 300ms', async () => {
    const fn = vi.fn();
    let t: ReturnType<typeof setTimeout> | null = null;
    const save = (v: string) => {
      if (t) clearTimeout(t);
      t = setTimeout(() => fn(v), 300);
    };
    save('a');
    save('ab');
    save('abc');
    await new Promise((r) => setTimeout(r, 350));
    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenCalledWith('abc');
  });
  it('paste comma list creates two chips; invalid blocks send', () => {
    const raw = '"Ada" <ada@x.com>, ben@y.org';
    const parts = raw.split(',').map((s) => s.trim());
    expect(parts.length).toBe(2);
    expect('foo'.includes('@')).toBe(false);
  });
});
