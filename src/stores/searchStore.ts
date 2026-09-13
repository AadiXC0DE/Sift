import { create } from 'zustand';
import type { ThreadRow, View } from '../app/ipc/types';
import { api } from '../app/ipc/commands';
import { useView } from './viewStore';
import { useAccounts } from './accountsStore';

/**
 * One visible search state (P7.2).
 *
 * `SearchInput` used to keep a private stash in a ref and never told the view
 * store about it, so `restore()` was a no-op and a debounce scheduled before an
 * immediate clear still applied the cleared query. The search state — query,
 * scope, generation, results, cursor, loading, error and the mailbox context
 * the search was entered from — now lives here, and the debounce lives at
 * module scope so it can be cancelled from empty input, Escape, unmount and a
 * scope change alike.
 */

export type SearchScope = 'local' | 'server';

/**
 * The mailbox the search started from. Escape has to put the user back exactly
 * where they were: same view, same account scope, same anchor row and offset,
 * same focused row.
 */
export interface SearchOrigin {
  view: View;
  accountScope: string | 'all';
  anchorKey: string | null;
  anchorOffset: number;
  focusedKey: string | null;
}

interface SearchState {
  query: string;
  /** `local` reads the downloaded window, `server` reads Gmail (P7.2). */
  scope: SearchScope;
  /** Bumped per request; a response for an older generation is dropped. */
  generation: number;
  /** Results of the completed Gmail search (server scope only). */
  results: ThreadRow[];
  cursor?: string;
  loading: boolean;
  error: string | null;
  origin: SearchOrigin | null;

  setQuery: (query: string) => void;
  /** Remember where the search started; the first origin wins. */
  begin: (origin: SearchOrigin) => void;
  /** Enter the local (downloaded mail) scope. */
  startLocal: () => void;
  /** Enter the Gmail scope and mark the request as in flight. */
  startServer: () => void;
  succeed: (generation: number, results: ThreadRow[], cursor?: string) => void;
  fail: (generation: number, message: string) => void;
  /** Leave search, reset the state and hand back the origin to restore. */
  leave: () => SearchOrigin | null;
}

const EMPTY = {
  query: '',
  scope: 'local' as SearchScope,
  results: [] as ThreadRow[],
  cursor: undefined,
  loading: false,
  error: null,
  origin: null,
};

export const useSearch = create<SearchState>((set, get) => ({
  ...EMPTY,
  generation: 0,
  setQuery: (query) => set({ query }),
  begin: (origin) => set((s) => (s.origin ? {} : { origin })),
  // The origin is deliberately preserved: a request running for the current
  // query must never lose the mailbox Escape has to restore.
  startLocal: () =>
    set((s) => ({
      ...EMPTY,
      query: s.query,
      origin: s.origin,
      scope: 'local',
      generation: s.generation + 1,
    })),
  startServer: () =>
    set((s) => ({
      ...EMPTY,
      query: s.query,
      origin: s.origin,
      scope: 'server',
      loading: true,
      generation: s.generation + 1,
    })),
  succeed: (generation, results, cursor) =>
    set((s) => (s.generation === generation ? { results, cursor, loading: false, error: null } : {})),
  fail: (generation, message) =>
    set((s) => (s.generation === generation ? { results: [], loading: false, error: message } : {})),
  leave: () => {
    const origin = get().origin;
    set((s) => ({ ...EMPTY, generation: s.generation + 1 }));
    return origin;
  },
}));

/** Debounce for the local (downloaded mail) search. */
export const SEARCH_DEBOUNCE_MS = 40;

/** DOM and Node typings disagree on the timer handle, so it is named here. */
type TimerHandle = ReturnType<typeof setTimeout>;
let timer: TimerHandle | undefined;

export function cancelSearchTimer(): void {
  clearTimeout(timer);
  timer = undefined;
}

function errorMessage(e: unknown): string {
  if (e && typeof e === 'object' && 'message' in e) {
    const msg = (e as { message: unknown }).message;
    if (typeof msg === 'string' && msg) return msg;
  }
  return 'Gmail search failed.';
}

function accountIds(): string[] {
  const scope = useView.getState().accountScope;
  return scope === 'all' ? useAccounts.getState().includedIds() : [scope];
}

/**
 * Switch the list to the downloaded-mail search. The visible rows stay the
 * list window's — it queries the same store through `threads_query` — so there
 * is exactly one result state on screen.
 */
export function applyLocalSearch(query: string): void {
  const q = query.trim();
  if (!q) return;
  useSearch.getState().startLocal();
  useView.getState().setView({ kind: 'search', q });
}

/** One debounced local request per current query (P7.2). */
export function scheduleLocalSearch(query: string): void {
  cancelSearchTimer();
  if (!query.trim()) return;
  timer = setTimeout(() => {
    timer = undefined;
    // The field may have moved on (or been cleared) while the timer ran: only
    // the current query may switch the view.
    if (useSearch.getState().query !== query) return;
    applyLocalSearch(query);
  }, SEARCH_DEBOUNCE_MS);
}

/**
 * Enter sends the query to Gmail: the completed response replaces the window as
 * the visible result set, and a provider failure is shown with Retry instead of
 * silently keeping local rows.
 */
export function startServerSearch(): void {
  cancelSearchTimer();
  const store = useSearch.getState();
  const query = store.query.trim();
  if (!query) return;
  store.startServer();
  useView.getState().setView({ kind: 'search', q: query });
  const generation = useSearch.getState().generation;
  try {
    const raw = localStorage.getItem('sift-recent-search') ?? '[]';
    const arr: string[] = JSON.parse(raw);
    localStorage.setItem(
      'sift-recent-search',
      JSON.stringify([query, ...arr.filter((x) => x !== query)].slice(0, 10)),
    );
  } catch {
    /* recent searches are best effort */
  }
  void api
    .search(accountIds(), query, 'server')
    .then((page) => useSearch.getState().succeed(generation, page.rows, page.nextCursor))
    .catch((e) => useSearch.getState().fail(generation, errorMessage(e)));
}
