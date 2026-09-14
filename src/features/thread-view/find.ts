/**
 * Reader-local find (P9.3).
 *
 * A mail body lives in a `sandbox="allow-scripts"` iframe with an opaque
 * origin, so the app document can neither call into it nor read what it
 * matched. Everything therefore travels as postMessage: a request goes into
 * the frame, the frame's shim answers with a count and the 1-based index of the
 * match it scrolled to. This module owns the three things that must outlive a
 * single render — the registered frames, the request channel they subscribe to,
 * and the pending answer per frame — so a frame that never answers (navigated
 * away, crashed, or gone before the body loaded) degrades to "0 matches"
 * instead of leaving the find bar spinning forever.
 */

export interface FindHit {
  count: number;
  index: number;
}

export interface FrameFinder {
  messageId: string;
  find(q: string, direction: 1 | -1, reset: boolean): Promise<FindHit>;
  clear(): void;
}

export interface FindRequest {
  messageId: string;
  q: string;
  direction: 1 | -1;
  reset: boolean;
}

/**
 * How long a frame may take to answer before find gives up on it. A find bar
 * that waits on a document that is already gone is worse than one that reports
 * nothing, and the next keystroke asks again anyway.
 */
const FIND_TIMEOUT_MS = 1500;

/** No matches: the answer used for an empty query, a timeout, or a lost frame. */
const NO_HIT: FindHit = { count: 0, index: 0 };

/** DOM and Node typings disagree on the timer handle, so it is named here. */
type TimerHandle = ReturnType<typeof setTimeout>;

interface PendingFind {
  resolve: (hit: FindHit) => void;
  timer: TimerHandle;
}

const finders = new Map<string, FrameFinder>();
const pending = new Map<string, PendingFind>();
const requestListeners = new Set<(req: FindRequest) => void>();
const focusListeners = new Set<(messageId: string) => void>();
/** The frame the user was last reading: what Cmd+F should search. */
let intended: string | null = null;

function settle(messageId: string, hit: FindHit): void {
  const entry = pending.get(messageId);
  if (!entry) return;
  pending.delete(messageId);
  clearTimeout(entry.timer);
  entry.resolve(hit);
}

/** Hands one request to the frame that owns `messageId`, if it is mounted. */
function emit(req: FindRequest): void {
  for (const listener of [...requestListeners]) listener(req);
}

/**
 * The finder a frame registers on mount. `messageId` is the only thing the
 * registry needs: posting happens through `subscribeFindRequests`, so a frame
 * that re-renders a new `srcdoc` keeps the same finder and the same promise
 * contract.
 */
export function createFrameFinder(messageId: string): FrameFinder {
  return {
    messageId,
    find(q, direction, reset) {
      // Starting a new search drops whatever was still in flight for this
      // frame: a superseded request must not keep a timer alive.
      settle(messageId, NO_HIT);
      return new Promise<FindHit>((resolve) => {
        const timer = setTimeout(() => {
          pending.delete(messageId);
          resolve(NO_HIT);
        }, FIND_TIMEOUT_MS);
        pending.set(messageId, { resolve, timer });
        emit({ messageId, q, direction, reset });
      });
    },
    clear() {
      // Fire and forget: closing find must not wait on a frame, and the answer
      // to a clear is the same empty result the next search would produce.
      emit({ messageId, q: '', direction: 1, reset: true });
    },
  };
}

/** Registers a frame's finder; the returned function unregisters it. */
export function registerFinder(f: FrameFinder): () => void {
  finders.set(f.messageId, f);
  return () => {
    if (finders.get(f.messageId) === f) finders.delete(f.messageId);
    if (intended === f.messageId) intended = null;
    // A reader that closes mid-search answers for itself, so the find bar never
    // waits on a frame that no longer exists.
    settle(f.messageId, NO_HIT);
  };
}

export function listFinders(): FrameFinder[] {
  return [...finders.values()];
}

/**
 * Marks `messageId` as the frame find should search — set when the user focuses
 * a message and when a frame asks for find with Cmd/Ctrl+F.
 */
export function focusFinder(messageId: string): void {
  intended = messageId;
  for (const listener of [...focusListeners]) listener(messageId);
}

/**
 * Escape pressed inside a frame is relayed here as a synthetic keydown on the
 * app window, so it travels the same path as an app-level Escape: the overlay
 * stack sees the find bar on top and closes it.
 */
export function escapeFinder(): void {
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
}

/** Subscribes to find requests; a frame posts the ones addressed to it. */
export function subscribeFindRequests(fn: (req: FindRequest) => void): () => void {
  requestListeners.add(fn);
  return () => {
    requestListeners.delete(fn);
  };
}

/** Delivers a frame's answer to the find() promise waiting on it. */
export function resolveFindResult(messageId: string, hit: FindHit): void {
  const count = Math.max(0, Math.floor(hit.count));
  const index = count === 0 ? 0 : Math.min(count, Math.max(1, Math.floor(hit.index)));
  settle(messageId, { count, index });
}

export function subscribeFrameFocus(fn: (messageId: string) => void): () => void {
  focusListeners.add(fn);
  return () => {
    focusListeners.delete(fn);
  };
}

/** The frame find should search, or null when it is gone. */
export function intendedFindTarget(): string | null {
  return intended && finders.has(intended) ? intended : null;
}

/** Test-only reset: drops in-flight searches and the intended target. */
export function clearFindResults(): void {
  for (const messageId of [...pending.keys()]) settle(messageId, NO_HIT);
  intended = null;
}
