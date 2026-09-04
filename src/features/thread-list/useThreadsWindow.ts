import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import { on } from '../../app/ipc/events';
import type { ThreadRow, ThreadsQuery, View } from '../../app/ipc/types';
import { useAccounts } from '../../stores/accountsStore';
import { useView } from '../../stores/viewStore';

interface WindowState {
  rows: ThreadRow[];
  nextCursor?: string;
  loading: boolean;
  generation: number;
}

// Re-query only affected windows; stale responses dropped via generation.
export function useThreadsWindow(view: View, opts?: { unreadOnly?: boolean; hasAttachment?: boolean }) {
  const scope = useView((s) => s.accountScope);
  const includedIds = useAccounts((s) => s.includedIds);
  const [state, setState] = useState<WindowState>({ rows: [], loading: true, generation: 0 });
  const gen = useRef(0);
  const key = JSON.stringify({ view, scope, opts });

  const load = useCallback(
    async (cursor?: string, append = false) => {
      const g = ++gen.current;
      if (!append) setState((s) => ({ ...s, loading: true }));
      try {
        const ids = includedIds();
        const accountIds = scope === 'all' ? ids : [scope];
        if (!accountIds.length) {
          setState({ rows: [], loading: false, generation: g });
          return;
        }
        const q: ThreadsQuery = {
          accountIds,
          view,
          cursor,
          limit: 100,
          unread_only: opts?.unreadOnly,
          has_attachment: opts?.hasAttachment,
        };
        const page = await api.threads_query(q);
        if (gen.current !== g) return; // stale
        setState((s) => ({
          rows: append ? [...s.rows, ...page.rows] : page.rows,
          nextCursor: page.nextCursor,
          loading: false,
          generation: g,
        }));
      } catch {
        if (gen.current === g) setState((s) => ({ ...s, loading: false }));
      }
    },
    // key encodes view+scope+opts; includedIds is a stable store selector
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [key],
  );

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    let raf = 0;
    let pending: string[] = [];
    const flush = () => {
      // Re-query affected page only: if changed id in loaded rows or first page, reload first page
      const ids = new Set(state.rows.map((r) => r.id));
      const hit = pending.some((id) => ids.has(id));
      pending = [];
      if (hit || state.rows.length < 100) load();
    };
    let unsub = () => {};
    on<{ thread_ids: string[] }>('store:threads', (p) => {
      const ids = (p as unknown as { thread_ids: string[] }).thread_ids ?? [];
      if (!ids.length) {
        load();
        return;
      }
      pending.push(...ids);
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(flush);
    })
      .then((u) => {
        unsub = u;
      })
      .catch(() => {});
    return () => {
      cancelAnimationFrame(raf);
      unsub();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load, state.rows.length]);

  const loadMore = useCallback(() => {
    if (state.nextCursor && !state.loading) load(state.nextCursor, true);
  }, [state.nextCursor, state.loading, load]);

  return { ...state, loadMore, reload: () => load() };
}
