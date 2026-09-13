import { describe, expect, it } from 'vitest';
import { activeKeyScopes } from './scopes';

describe('P3.5 modal scopes win', () => {
  it('drops list/thread scopes while an overlay is open', () => {
    expect(activeKeyScopes({ paletteOpen: false, modalOpen: true, threadOpen: true })).toEqual(['global']);
    expect(activeKeyScopes({ paletteOpen: true, modalOpen: false, threadOpen: false })).toEqual([
      'global',
      'palette',
    ]);
  });

  it('activates list, and thread innermost, when nothing overlays them', () => {
    expect(activeKeyScopes({ paletteOpen: false, modalOpen: false, threadOpen: false })).toEqual([
      'global',
      'list',
    ]);
    expect(activeKeyScopes({ paletteOpen: false, modalOpen: false, threadOpen: true })).toEqual([
      'global',
      'thread',
      'list',
    ]);
  });
});
