import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import { on } from '../../app/ipc/events';
import type { ThreadRow, ThreadsQuery, View } from '../../app/ipc/types';
import { useAccounts } from '../../stores/accountsStore';
import { useSelection } from '../../stores/selectionStore';
import { useView } from '../../stores/viewStore';
import { COALESCE_MS, MAX_WINDOW, PAGE_SIZE, appendPage, capWindow, dedupeRows } from './threadWindow';

export interface ThreadsWindow {
  rows: ThreadRow[];
  nextCursor?: string;
  initialLoading: boolean;
  loadingMore: boolean;
  error: string | null;
  queryGeneration: number;
  /** Bumped when a head/range refresh replaces rows, so scroll can re-anchor. */
  refreshRevision: number;
}

export interface ThreadsWindowResult extends ThreadsWindow {
  loadMore: () => void;
  reload: () => void;
}

function errorMessage(e: unknown): string {
  if (e && typeof e === 'object' && 'message' in e) {
    const msg = (e as { message: unknown }).message;
    if (typeof msg === 'string' && msg) return msg;
  }
  return 'Could not load conversations.';
}

/**
 * Deterministic list window.
 *
 * The query identity is sorted included account IDs + view + filters + sort
 * mode. Loads never mix identities: results are dropped when the query
 * generation or the requested cursor no longer matches, and loaded rows are
 * cleared on a query change so an old scope is never actionable. Store events
 * refresh the head page (a bounded range) without collapsing loaded pages or
 * the virtualizer anchor.
 */
export function useThreadsWindow(
  view: View,
  opts?: { unreadOnly?: boolean; hasAttachment?: boolean },
): ThreadsWindowResult {
  const scope = useView((s) => s.accountScope);
  const accounts = useAccounts((s) => s.accounts);
  const included = useAccounts((s) => s.included);

  const accountIds = useMemo(() => {
    const ids = scope === 'all' ? accounts.filter((a) => included[a.id] !== false).map((a) => a.id) : [scope];
    return [...new Set(ids)].sort();
  }, [scope, accounts, included]);

  const unreadOnly = !!opts?.unreadOnly;
  const hasAttachment = !!opts?.hasAttachment;
  // Sort mode is part of the query key; `threads_query` currently always sorts
  // by last message, so it is carried through the key only.
  const sortMode = 'lastMessageAt';
  const queryKey = useMemo(
    () => JSON.stringify({ accountIds, view, unreadOnly, hasAttachment, sortMode }),
    [accountIds, view, unreadOnly, hasAttachment, sortMode],
  );

  const [state, setState] = useState<ThreadsWindow>({
    rows: [],
    nextCursor: undefined,
    initialLoading: true,
    loadingMore: false,
    error: null,
    queryGeneration: 0,
    refreshRevision: 0,
  });
  const [reloadNonce, setReloadNonce] = useState(0);

  const genRef = useRef(0);
  const keyRef = useRef(queryKey);
  const viewRef = useRef(view);
  viewRef.current = view;
  const filtersRef = useRef({ unreadOnly, hasAttachment });
  filtersRef.current = { unreadOnly, hasAttachment };
  const accountIdsRef = useRef(accountIds);
  accountIdsRef.current = accountIds;
  const stateRef = useRef(state);
  stateRef.current = state;
  const loadingMoreRef = useRef(false);
  const inflightCursorRef = useRef<string | undefined>(undefined);

  const buildQuery = useCallback((cursor: string | undefined, limit: number): ThreadsQuery => {
    const filters = filtersRef.current;
    return {
      accountIds: accountIdsRef.current,
      view: viewRef.current,
      cursor,
      limit,
      unread_only: filters.unreadOnly,
      has_attachment: filters.hasAttachment,
    };
  }, []);

  const fetchPage = useCallback(
    async (cursor: string | undefined, mode: 'initial' | 'more', generation: number, limit = PAGE_SIZE) => {
      if (!accountIdsRef.current.length) {
        const acc = useAccounts.getState();
        // Stay in loading until accounts exist so first paint isn't an empty inbox.
        if (genRef.current === generation) {
          setState((s) => ({
            ...s,
            initialLoading: acc.loading || acc.accounts.length === 0,
            loadingMore: false,
          }));
        }
        return;
      }
      if (mode === 'more') {
        if (loadingMoreRef.current) return; // one request per cursor
        loadingMoreRef.current = true; // synchronous, before the await
        inflightCursorRef.current = cursor;
        setState((s) => ({ ...s, loadingMore: true, error: null }));
      } else {
        inflightCursorRef.current = cursor;
      }
      try {
        const page = await api.threads_query(buildQuery(cursor, limit));
        if (genRef.current !== generation) return; // query changed underneath
        if (mode === 'more' && inflightCursorRef.current !== cursor) return; // requested cursor differs
        if (mode === 'more') {
          loadingMoreRef.current = false;
          inflightCursorRef.current = undefined;
        }
        const fresh = dedupeRows(page.rows);
        setState((s) => {
          if (mode === 'more') {
            const capped = capWindow(appendPage(s.rows, fresh));
            return { ...s, rows: capped.rows, nextCursor: page.nextCursor, loadingMore: false, error: null };
          }
          return {
            ...s,
            rows: fresh,
            nextCursor: page.nextCursor,
            initialLoading: false,
            loadingMore: false,
            error: null,
            queryGeneration: generation,
          };
        });
      } catch (e) {
        if (genRef.current !== generation) return;
        if (mode === 'more') {
          loadingMoreRef.current = false;
          inflightCursorRef.current = undefined;
        }
        setState((s) => ({ ...s, initialLoading: false, loadingMore: false, error: errorMessage(e) }));
      }
    },
    [buildQuery],
  );

  // Single entry point: the initial load and every query identity change. Old
  // rows are cleared immediately so stale rows are never actionable.
  useEffect(() => {
    const generation = ++genRef.current;
    keyRef.current = queryKey;
    loadingMoreRef.current = false;
    inflightCursorRef.current = undefined;
    useSelection.getState().clearSelection();
    setState({
      rows: [],
      nextCursor: undefined,
      initialLoading: true,
      loadingMore: false,
      error: null,
      queryGeneration: generation,
      refreshRevision: 0,
    });
    void fetchPage(undefined, 'initial', generation);
  }, [queryKey, reloadNonce, fetchPage]);

  /**
   * Refresh without collapsing the window: query the same number of rows we
   * have loaded (page-aligned, bounded by the cap) so the refreshed range is
   * authoritative for updates and removals while the loaded page boundaries
   * survive. The returned cursor is revalidated before the next append.
   */
  const refreshWindow = useCallback(async () => {
    const generation = genRef.current;
    const key = keyRef.current;
    if (!accountIdsRef.current.length) return;
    const loaded = stateRef.current.rows;
    const limit = Math.min(MAX_WINDOW, Math.max(PAGE_SIZE, Math.ceil(loaded.length / PAGE_SIZE) * PAGE_SIZE));
    try {
      const page = await api.threads_query(buildQuery(undefined, limit));
      if (genRef.current !== generation || keyRef.current !== key) return; // stale generation
      const rows = dedupeRows(page.rows);
      setState((s) => ({
        ...s,
        rows,
        nextCursor: page.nextCursor,
        initialLoading: false,
        error: null,
        refreshRevision: s.refreshRevision + 1,
      }));
    } catch (e) {
      if (genRef.current !== generation) return;
      setState((s) => ({ ...s, error: errorMessage(e) }));
    }
  }, [buildQuery]);

  // Store events: filtered by account, coalesced, and handled from a stable
  // callback that reads live refs so equal-length replacements cannot leave
  // stale membership.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let unsub: (() => void) | undefined;
    let disposed = false;
    const flush = () => {
      timer = undefined;
      void refreshWindow();
    };
    on<{ account_id?: string; thread_ids?: string[] }>('store:threads', (payload) => {
      const accountId = payload?.account_id ?? '';
      // An empty account id means "reload everything"; otherwise only events
      // from accounts in this query are relevant, even for brand-new thread ids.
      if (accountId !== '' && !accountIdsRef.current.includes(accountId)) return;
      clearTimeout(timer);
      timer = setTimeout(flush, COALESCE_MS);
    })
      .then((u) => {
        if (disposed) u();
        else unsub = u;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      clearTimeout(timer);
      unsub?.();
    };
  }, [refreshWindow]);

  const loadMore = useCallback(() => {
    const current = stateRef.current;
    if (!current.nextCursor || current.initialLoading || current.loadingMore) return;
    void fetchPage(current.nextCursor, 'more', genRef.current);
  }, [fetchPage]);

  const reload = useCallback(() => setReloadNonce((n) => n + 1), []);

  return { ...state, loadMore, reload };
}
