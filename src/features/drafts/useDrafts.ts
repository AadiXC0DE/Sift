import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import { on } from '../../app/ipc/events';
import type { Draft } from '../../app/ipc/types';

export interface DraftsWindow {
  rows: Draft[];
  nextCursor?: string;
  initialLoading: boolean;
  loadingMore: boolean;
  error: string | null;
}

export interface DraftsWindowResult extends DraftsWindow {
  loadMore: () => void;
  reload: () => void;
}

const PAGE_SIZE = 30;
// `drafts_list` rejects limits above 100, so a deep reload cannot ask for more.
const MAX_PAGE_SIZE = 100;
// A save touches the store several times (upsert, remote push, state change);
// coalescing them keeps a burst of save events to a single head refresh.
const COALESCE_MS = 150;

function errorMessage(e: unknown): string {
  if (e && typeof e === 'object' && 'message' in e) {
    const msg = e.message;
    if (typeof msg === 'string' && msg) return msg;
  }
  return 'Could not load drafts.';
}

function draftKey(d: Draft): string {
  return `${d.accountId}:${d.localId}`;
}

/** Drop duplicate keys, keeping the first server occurrence. */
function dedupeDrafts(drafts: Draft[]): Draft[] {
  const seen = new Set<string>();
  const out: Draft[] = [];
  for (const d of drafts) {
    const key = draftKey(d);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(d);
  }
  return out;
}

/** Append a page, preserving server order and never duplicating a loaded draft. */
function appendDrafts(current: Draft[], page: Draft[]): Draft[] {
  const seen = new Set(current.map(draftKey));
  const out = current.slice();
  for (const d of page) {
    const key = draftKey(d);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(d);
  }
  return out;
}

/**
 * Draft list window for the drafts view.
 *
 * The query identity is the sorted, de-duplicated account id set. A response
 * from a previous identity is dropped by generation, and loaded rows are
 * cleared the moment the identity changes so an old scope is never actionable.
 * Store events refresh the head page (as many rows as are already loaded) so a
 * draft saved by the composer shows up without collapsing the loaded pages.
 */
export function useDrafts(accountIds: string[], opts?: { limit?: number }): DraftsWindowResult {
  const limit = Math.min(MAX_PAGE_SIZE, Math.max(1, Math.floor(opts?.limit ?? PAGE_SIZE)));

  const ids = useMemo(() => [...new Set(accountIds)].sort(), [accountIds]);
  const idsKey = ids.join('\u0000');

  const [state, setState] = useState<DraftsWindow>({
    rows: [],
    nextCursor: undefined,
    initialLoading: false,
    loadingMore: false,
    error: null,
  });
  const [reloadNonce, setReloadNonce] = useState(0);

  const genRef = useRef(0);
  const idsRef = useRef(ids);
  idsRef.current = ids;
  const stateRef = useRef(state);
  stateRef.current = state;
  const loadingMoreRef = useRef(false);
  const inflightCursorRef = useRef<string | undefined>(undefined);

  const fetchPage = useCallback(
    async (cursor: string | undefined, mode: 'initial' | 'more', generation: number, pageLimit = limit) => {
      if (!idsRef.current.length) {
        // Nothing to query: an empty scope is a settled empty list, not a load.
        if (genRef.current === generation) {
          setState((s) => ({ ...s, initialLoading: false, loadingMore: false }));
        }
        return;
      }
      if (mode === 'more') {
        if (loadingMoreRef.current) return; // one request per cursor
        loadingMoreRef.current = true; // synchronous, before the await
        inflightCursorRef.current = cursor;
        setState((s) => ({ ...s, loadingMore: true, error: null }));
      }
      try {
        const page = await api.drafts_list({
          accountIds: idsRef.current,
          cursor,
          limit: pageLimit,
        });
        if (genRef.current !== generation) return; // identity changed underneath
        if (mode === 'more') {
          if (inflightCursorRef.current !== cursor) return; // requested cursor differs
          loadingMoreRef.current = false;
          inflightCursorRef.current = undefined;
        }
        const fresh = dedupeDrafts(page.drafts);
        setState((s) => {
          if (mode === 'more') {
            return {
              ...s,
              rows: appendDrafts(s.rows, fresh),
              nextCursor: page.nextCursor,
              loadingMore: false,
              error: null,
            };
          }
          return {
            ...s,
            rows: fresh,
            nextCursor: page.nextCursor,
            initialLoading: false,
            loadingMore: false,
            error: null,
          };
        });
      } catch (e) {
        if (genRef.current !== generation) return;
        if (mode === 'more') {
          loadingMoreRef.current = false;
          inflightCursorRef.current = undefined;
        }
        // An error is never a successful empty page: the error stays visible
        // and the loaded rows (if any) survive.
        setState((s) => ({
          ...s,
          initialLoading: false,
          loadingMore: false,
          error: errorMessage(e),
        }));
      }
    },
    [limit],
  );

  // Single entry point: the initial load and every identity change.
  useEffect(() => {
    const generation = ++genRef.current;
    loadingMoreRef.current = false;
    inflightCursorRef.current = undefined;
    const hasAccounts = idsRef.current.length > 0;
    setState({
      rows: [],
      nextCursor: undefined,
      initialLoading: hasAccounts,
      loadingMore: false,
      error: null,
    });
    if (!hasAccounts) return;
    void fetchPage(undefined, 'initial', generation);
    // `idsKey` is the identity; `idsRef` always holds the matching ids.
  }, [idsKey, reloadNonce, fetchPage]);

  /**
   * Refresh without collapsing the window: re-query the number of rows already
   * loaded (page-aligned, bounded) so a newly saved draft lands at the head
   * while the loaded page boundaries survive.
   */
  const refreshHead = useCallback(async () => {
    const generation = genRef.current;
    if (!idsRef.current.length) return;
    const loaded = stateRef.current.rows.length;
    const pages = Math.max(1, Math.ceil(loaded / limit));
    const pageLimit = Math.min(MAX_PAGE_SIZE, pages * limit);
    try {
      const page = await api.drafts_list({
        accountIds: idsRef.current,
        cursor: undefined,
        limit: pageLimit,
      });
      if (genRef.current !== generation) return; // identity changed underneath
      setState((s) => ({
        ...s,
        rows: dedupeDrafts(page.drafts),
        nextCursor: page.nextCursor,
        initialLoading: false,
        error: null,
      }));
    } catch (e) {
      if (genRef.current !== generation) return;
      setState((s) => ({ ...s, error: errorMessage(e) }));
    }
  }, [limit]);

  // Store events are filtered by account, coalesced, and read live refs so the
  // subscription survives identity changes without re-subscribing.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let unsub: (() => void) | undefined;
    let disposed = false;
    const flush = () => {
      timer = undefined;
      void refreshHead();
    };
    on<{ account_id?: string; draft_id?: string }>('store:drafts', (payload) => {
      const accountId = payload?.account_id ?? '';
      // An empty account id means "reload everything"; otherwise only events
      // from accounts in this scope matter.
      if (accountId !== '' && !idsRef.current.includes(accountId)) return;
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
  }, [refreshHead]);

  const loadMore = useCallback(() => {
    const current = stateRef.current;
    if (!current.nextCursor || current.initialLoading || current.loadingMore) return;
    void fetchPage(current.nextCursor, 'more', genRef.current);
  }, [fetchPage]);

  const reload = useCallback(() => setReloadNonce((n) => n + 1), []);

  return { ...state, loadMore, reload };
}
