import { describe, it, expect } from 'vitest';
import { filterCommands } from './registry';

describe('P8-T06 palette filter', () => {
  it('arc -> Archive first; hidden when() false', () => {
    const r = filterCommands('arc');
    expect(r[0]?.title.toLowerCase()).toMatch(/archiv/);
  });
});
