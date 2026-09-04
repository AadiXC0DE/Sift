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
