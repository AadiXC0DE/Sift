import { afterEach, describe, expect, it } from 'vitest';
import { clearSurfaces, handleEscape, hasBlockingSurface, pushSurface, topSurface } from './overlayStack';

afterEach(() => clearSurfaces());

describe('P9.5 overlay surfaces', () => {
  it('closes only the topmost surface per Escape', () => {
    const closed: string[] = [];
    pushSurface({ kind: 'managed', close: () => closed.push('settings') });
    pushSurface({ kind: 'managed', close: () => closed.push('label-picker') });

    expect(handleEscape()).toBe('handled');
    expect(closed).toEqual(['label-picker']);
    expect(handleEscape()).toBe('handled');
    expect(closed).toEqual(['label-picker', 'settings']);
    expect(handleEscape()).toBe('none');
  });

  it('lets a self-dismissing surface handle its own Escape', () => {
    pushSurface({ kind: 'managed', close: () => {} });
    pushSurface({ kind: 'native' });
    expect(handleEscape()).toBe('pass');
    // The managed surface underneath is untouched: one key, one surface.
    expect(topSurface()?.kind).toBe('native');
    expect(hasBlockingSurface()).toBe(true);
  });

  it('reports the keyboard as blocked while any surface is open', () => {
    expect(hasBlockingSurface()).toBe(false);
    const remove = pushSurface({ kind: 'native' });
    expect(hasBlockingSurface()).toBe(true);
    remove();
    expect(hasBlockingSurface()).toBe(false);
    expect(handleEscape()).toBe('none');
  });

  it('removes a surface by identity, so an early close cannot pop a later one', () => {
    const removeFirst = pushSurface({ kind: 'managed', close: () => {} });
    pushSurface({ kind: 'managed', close: () => {} });
    removeFirst();
    expect(topSurface()).toBeDefined();
    removeFirst(); // idempotent
    expect(topSurface()).toBeDefined();
  });
});
