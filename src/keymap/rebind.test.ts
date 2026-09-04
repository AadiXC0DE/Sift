import { describe, it, expect } from 'vitest';
import { KeymapEngine } from './engine';

describe('P9-T03 rebind conflicts + presets', () => {
  it('conflict detected when two actions share key in same scope', () => {
    const e = new KeymapEngine();
    e.register([
      { key: 'e', scope: 'list', action: 'archive' },
      { key: 'e', scope: 'list', action: 'snooze' },
    ]);
    expect(e.bindings.get('list:e')?.length).toBe(2);
  });
  it('gmail preset y archives', async () => {
    const m = await import('./presets/gmail');
    expect(m.bindings.find((b) => b.key === 'y')?.action).toBe('archive');
  });
});
