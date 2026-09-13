import { describe, it, expect, vi, beforeEach } from 'vitest';
import { KeymapEngine } from './engine';
import { defaultBindings } from './defaults';

function key(k: string, opts: Partial<KeyboardEventInit> = {}): KeyboardEvent {
  return new KeyboardEvent('keydown', { key: k, bubbles: true, ...opts });
}

describe('P1-T07 sequences and input guard', () => {
  let e: KeymapEngine;
  beforeEach(() => {
    e = new KeymapEngine();
    e.register(defaultBindings);
  });
  it('g then i within 800ms fires goInbox', () => {
    const fn = vi.fn();
    e.onAction = fn;
    e.handle(key('g'), ['global']);
    e.handle(key('i'), ['global']);
    expect(fn).toHaveBeenCalledWith('goInbox', 'g i');
  });
  it('after 900ms it does not fire (simulated by manual clear)', () => {
    const fn = vi.fn();
    e.onAction = fn;
    e.handle(key('g'), ['global']);
    e.clearSeq();
    e.handle(key('i'), ['global']);
    expect(fn).not.toHaveBeenCalledWith('goInbox', expect.anything());
  });
  it('typing in input does not fire e', () => {
    const fn = vi.fn();
    e.onAction = fn;
    const input = document.createElement('input');
    document.body.appendChild(input);
    const ev = key('e');
    Object.defineProperty(ev, 'target', { value: input });
    e.handle(ev, ['list']);
    expect(fn).not.toHaveBeenCalled();
    input.remove();
  });
});

describe('P1-T08 scope precedence', () => {
  it('thread binding for o beats list when thread active', () => {
    const eng = new KeymapEngine();
    eng.register([
      { key: 'o', scope: 'list', action: 'open' },
      { key: 'o', scope: 'thread', action: 'toggleMsg' },
    ]);
    expect(eng.lookup('o', ['list', 'thread'])?.action).toBe('toggleMsg');
    expect(eng.lookup('o', ['list'])?.action).toBe('open');
  });
});

describe('P3.5 scoped action routing', () => {
  let e: KeymapEngine;
  beforeEach(() => {
    e = new KeymapEngine();
    e.register(defaultBindings);
  });

  it('never claims a key while an IME is composing', () => {
    const fn = vi.fn();
    e.subscribe(({ action }) => {
      fn(action);
      return true;
    });
    const ev = key('j', { isComposing: true } as Partial<KeyboardEventInit>);
    expect(e.handle(ev, ['global', 'list'])).toBe(false);
    expect(fn).not.toHaveBeenCalled();
  });

  it('dispatches to the innermost scope that owns the binding', () => {
    const seen: string[] = [];
    e.subscribe(({ action, scope }) => {
      seen.push(`${scope}:${action}`);
      return true;
    });
    e.handle(key('o'), ['global', 'list', 'thread']);
    e.handle(key('j'), ['global', 'list', 'thread']);
    expect(seen).toEqual(['thread:toggleMsg', 'list:focusNext']);
  });

  it('only the subscriber for the resolved scope runs', () => {
    const listHandler = vi.fn(() => true);
    const threadHandler = vi.fn(() => true);
    e.subscribe(({ scope }) => (scope === 'list' ? listHandler() : false));
    e.subscribe(({ scope }) => (scope === 'thread' ? threadHandler() : false));
    e.handle(key('e'), ['global', 'list', 'thread']);
    expect(threadHandler).toHaveBeenCalledTimes(1);
    expect(listHandler).not.toHaveBeenCalled();
    e.handle(key('e'), ['global', 'list']);
    expect(listHandler).toHaveBeenCalledTimes(1);
  });

  it('honours a remapped binding from a preset', async () => {
    const gmail = await import('./presets/gmail');
    const eng = new KeymapEngine();
    eng.register(gmail.bindings);
    const fn = vi.fn();
    eng.subscribe(({ action }) => {
      fn(action);
      return true;
    });
    eng.handle(key('y'), ['global', 'list']);
    expect(fn).toHaveBeenCalledWith('archive');
  });
});
