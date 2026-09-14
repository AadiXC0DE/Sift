import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { JSDOM } from 'jsdom';
import {
  clearFindResults,
  createFrameFinder,
  escapeFinder,
  focusFinder,
  intendedFindTarget,
  listFinders,
  registerFinder,
  resolveFindResult,
  subscribeFindRequests,
  type FindRequest,
} from './find';
import { buildShim } from './shim';

/** The token the shim was built with; the parent rejects any other. */
const TOKEN = 'tok-1234';

interface FrameMessage {
  __sift?: boolean;
  token?: string;
  type?: string;
  count?: number;
  index?: number;
}

interface ShimHarness {
  /** The frame's document, as the shim sees it. */
  dom: JSDOM;
  doc: Document;
  /** Everything the frame posted back to the app. */
  posted: FrameMessage[];
  /** Delivers a request the way the app posts it into the frame. */
  ask: (data: Record<string, unknown>, source?: unknown) => void;
  /** Runs a search and returns the frame's answer. */
  search: (q: string, direction?: 1 | -1, reset?: boolean) => FrameMessage | undefined;
  /** Presses a key in the frame and returns the message types it relayed. */
  keydown: (init: KeyboardEventInit) => string[];
  marks: () => NodeListOf<HTMLElement>;
}

/**
 * Runs the emitted shim in a real DOM. The shim is a string by necessity — it
 * ships into a sandboxed frame — so the only way to know what it does is to run
 * it. `window` and `parent` are injected, which is what lets the test watch
 * both directions of the protocol.
 */
function loadShim(bodyHtml: string): ShimHarness {
  const dom = new JSDOM(`<!doctype html><html><body>${bodyHtml}</body></html>`);
  const doc = dom.window.document;
  const posted: FrameMessage[] = [];
  const parent = {
    postMessage: (msg: FrameMessage) => {
      posted.push(msg);
    },
  };
  const listeners = new Map<string, (event: unknown) => void>();
  const win: {
    __siftFind?: unknown;
    addEventListener: (type: string, fn: (event: unknown) => void) => void;
  } = {
    addEventListener: (type, fn) => {
      listeners.set(type, fn);
    },
  };
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  const run = new Function(
    'window',
    'parent',
    'document',
    'NodeFilter',
    'ResizeObserver',
    buildShim('n1', TOKEN),
  );
  // The document comes from the harness's own realm; the shim only needs the
  // DOM values it was written against.
  run(win, parent, doc, NodeFilter, ResizeObserverStub);

  const ask = (data: Record<string, unknown>, source: unknown = parent) => {
    listeners.get('message')!({ source, data });
  };
  // Every request is answered with a find-result; the shim then re-reports its
  // height, so the result is not simply the last message on the wire.
  const result = () => posted.filter((m) => m.type === 'find-result').pop();
  const search = (q: string, direction: 1 | -1 = 1, reset = true) => {
    ask({ __siftFindReq: true, token: TOKEN, type: 'find', q, direction, reset });
    return result();
  };
  const keydown = (init: KeyboardEventInit) => {
    const before = posted.length;
    doc.dispatchEvent(new dom.window.KeyboardEvent('keydown', { bubbles: true, ...init }));
    return posted.slice(before).map((m) => m.type ?? '');
  };
  const marks = () => doc.querySelectorAll<HTMLElement>('mark.__sift-hl');

  return { dom, doc, posted, ask, search, keydown, marks };
}

let harnesses: ShimHarness[] = [];

beforeEach(() => {
  clearFindResults();
  harnesses = [];
});

afterEach(() => {
  vi.useRealTimers();
  for (const h of harnesses) h.dom.window.close();
});

function load(bodyHtml: string) {
  const h = loadShim(bodyHtml);
  harnesses.push(h);
  return h;
}

describe('P9.3 find registry', () => {
  it('resolves with no matches instead of hanging when a frame never answers', async () => {
    vi.useFakeTimers();
    const finder = createFrameFinder('m1');
    const unregister = registerFinder(finder);
    const settled = vi.fn();
    const pending = finder.find('budget', 1, true);
    void pending.then(settled);

    // The request goes out immediately; a navigated or crashed frame simply
    // never answers it.
    await vi.advanceTimersByTimeAsync(1400);
    expect(settled).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(200);
    await expect(pending).resolves.toEqual({ count: 0, index: 0 });
    unregister();
  });

  it('sends the query, direction and reset the caller asked for', async () => {
    const requests: FindRequest[] = [];
    const off = subscribeFindRequests((req) => requests.push(req));
    const finder = createFrameFinder('m1');
    const unregister = registerFinder(finder);

    // Next after a search steps without re-scanning; Previous asks backwards.
    const forward = finder.find('budget', 1, false);
    const backward = finder.find('budget', -1, false);
    expect(requests).toEqual([
      { messageId: 'm1', q: 'budget', direction: 1, reset: false },
      { messageId: 'm1', q: 'budget', direction: -1, reset: false },
    ]);

    // The superseded request cannot be left pending by the newer one.
    await expect(forward).resolves.toEqual({ count: 0, index: 0 });
    resolveFindResult('m1', { count: 4, index: 3 });
    await expect(backward).resolves.toEqual({ count: 4, index: 3 });

    off();
    unregister();
  });

  it('clears a frame without waiting for its answer', () => {
    const requests: FindRequest[] = [];
    const off = subscribeFindRequests((req) => requests.push(req));
    const finder = createFrameFinder('m1');
    const unregister = registerFinder(finder);

    finder.clear();
    expect(requests).toEqual([{ messageId: 'm1', q: '', direction: 1, reset: true }]);

    off();
    unregister();
  });

  it('keeps an answer the counter can show when a frame reports nonsense', async () => {
    const finder = createFrameFinder('m1');
    const unregister = registerFinder(finder);

    const past = finder.find('x', 1, true);
    resolveFindResult('m1', { count: 3, index: 9 });
    await expect(past).resolves.toEqual({ count: 3, index: 3 });

    const negative = finder.find('x', 1, true);
    resolveFindResult('m1', { count: -2, index: -1 });
    await expect(negative).resolves.toEqual({ count: 0, index: 0 });

    unregister();
  });

  it('answers for a frame that closes mid-search', async () => {
    const finder = createFrameFinder('m1');
    const unregister = registerFinder(finder);
    const pending = finder.find('x', 1, true);
    unregister();
    await expect(pending).resolves.toEqual({ count: 0, index: 0 });
  });

  it('searches the frame the user was last reading', () => {
    const first = createFrameFinder('m1');
    const second = createFrameFinder('m2');
    const unregisterFirst = registerFinder(first);
    const unregisterSecond = registerFinder(second);
    expect(listFinders().map((f) => f.messageId)).toEqual(['m1', 'm2']);
    expect(intendedFindTarget()).toBeNull();

    focusFinder('m2');
    expect(intendedFindTarget()).toBe('m2');

    // A frame that goes away cannot stay the target.
    unregisterSecond();
    expect(intendedFindTarget()).toBeNull();
    expect(listFinders().map((f) => f.messageId)).toEqual(['m1']);
    unregisterFirst();
    expect(listFinders()).toEqual([]);
  });

  it('relays Escape from a frame onto the app window', () => {
    const seen = vi.fn();
    window.addEventListener('keydown', seen);
    escapeFinder();
    window.removeEventListener('keydown', seen);
    expect(seen).toHaveBeenCalledTimes(1);
    expect((seen.mock.calls[0][0] as KeyboardEvent).key).toBe('Escape');
  });
});

describe('P9.3 frame protocol', () => {
  it('counts every match in the message and marks the first as current', () => {
    const h = load('<p>cat</p><p>cat and Cat</p>');

    expect(h.search('cat')).toMatchObject({ type: 'find-result', count: 3, index: 1 });
    expect(h.marks()).toHaveLength(3);
    expect([...h.marks()].map((m) => m.getAttribute('data-sift-match'))).toEqual(['1', '2', '3']);
    expect(h.marks()[0].className).toBe('__sift-hl __sift-hl-active');
    expect(h.marks()[0].style.background).toBe('var(--sift-mark-active)');
    expect(h.marks()[1].className).toBe('__sift-hl');
  });

  it('matches case-insensitively without overlapping or re-matching its own marks', () => {
    const h = load('<p>aaaa</p>');

    // 'aa' twice, not three times: the matches must not overlap.
    expect(h.search('aaaa'.slice(0, 2))).toMatchObject({ count: 2, index: 1 });
    expect(h.marks()).toHaveLength(2);

    // Stepping must not wrap the highlight text into new matches.
    h.search('aa', 1, false);
    expect(h.marks()).toHaveLength(2);
    expect(h.doc.body.textContent).toBe('aaaa');
  });

  it('steps through matches and wraps at both ends', () => {
    const h = load('<p>cat</p><p>cat</p><p>cat</p>');

    expect(h.search('cat')).toMatchObject({ index: 1 });
    expect(h.search('cat', 1, false)).toMatchObject({ count: 3, index: 2 });
    expect(h.marks()[1].className).toBe('__sift-hl __sift-hl-active');
    expect(h.marks()[0].className).toBe('__sift-hl');
    // Past the last match, and before the first.
    expect(h.search('cat', 1, false)).toMatchObject({ index: 3 });
    expect(h.search('cat', 1, false)).toMatchObject({ index: 1 });
    expect(h.search('cat', -1, false)).toMatchObject({ index: 3 });
  });

  it('removes every mark and restores the text on an empty query', () => {
    const h = load('<p>cat sat</p>');
    h.search('cat');
    const wrapped = h.doc.body.innerHTML;

    expect(h.search('')).toMatchObject({ type: 'find-result', count: 0, index: 0 });
    expect(h.marks()).toHaveLength(0);
    expect(h.doc.body.textContent).toBe('cat sat');
    expect(h.doc.body.innerHTML).toBe('<p>cat sat</p>');
    expect(h.doc.body.innerHTML).not.toBe(wrapped);
  });

  it('never marks its own script, and reports a height after every search', () => {
    // The shim's script ships inside the body, beside the message: its text is
    // not content, and marking it would rewrite the frame's own code.
    const h = load('<p>find</p><script>var find = "find";</script>');
    expect(h.search('find')).toMatchObject({ count: 1, index: 1 });

    expect(h.doc.querySelectorAll('script mark')).toHaveLength(0);
    expect(h.doc.querySelector('script')!.textContent).toBe('var find = "find";');
    expect(h.posted.filter((m) => m.type === 'size').length).toBeGreaterThan(0);
  });

  it('ignores a request from another window or without this frame token', () => {
    const h = load('<p>cat</p>');

    h.ask({ __siftFindReq: true, token: TOKEN, type: 'find', q: 'cat', direction: 1, reset: true }, window);
    h.ask({ __siftFindReq: true, token: 'other', type: 'find', q: 'cat', direction: 1, reset: true });
    h.ask({ token: TOKEN, type: 'find', q: 'cat', direction: 1, reset: true });

    expect(h.posted).toEqual([]);
    expect(h.marks()).toHaveLength(0);
  });

  it('forwards Cmd/Ctrl+F out of the frame, always', () => {
    const h = load('<p>cat</p>');
    expect(h.keydown({ key: 'f', metaKey: true })).toEqual(['find-open']);
    expect(h.keydown({ key: 'f', ctrlKey: true })).toEqual(['find-open']);
    // A bare 'f' is message text, not a command.
    expect(h.keydown({ key: 'f' })).toEqual([]);
  });

  it('forwards Escape only while a find started by the app is live', () => {
    const h = load('<p>cat</p>');

    // Nothing is highlighted yet: Escape inside mail keeps its own meaning.
    expect(h.keydown({ key: 'Escape' })).toEqual([]);

    h.search('cat');
    expect(h.keydown({ key: 'Escape' })).toEqual(['find-escape']);

    // Clearing ends the find, so Escape stops leaving the frame.
    h.search('');
    expect(h.keydown({ key: 'Escape' })).toEqual([]);
  });
});
