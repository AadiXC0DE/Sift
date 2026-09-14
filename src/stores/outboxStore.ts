import { create } from 'zustand';
import { api } from '../app/ipc/commands';
import { readSiftError } from '../lib/siftError';
import type { OperationState, OutboxCounts, OutboxOp } from '../app/ipc/types';

/**
 * The panel lists exactly the work a person can act on. Done and cancelled rows
 * are history: the counts still report them, but rendering thousands of
 * finished operations would be a log, not an outbox (P6.6).
 */
export const OUTBOX_VISIBLE_STATES: OperationState[] = ['pending', 'inflight', 'uncertain', 'failed'];

/**
 * One page is what the panel shows at a time. 10,000 queued operations must not
 * become 10,000 rows: the list is paged by cursor and the DOM holds one page
 * plus whatever the user explicitly asked for.
 */
export const OUTBOX_PAGE_LIMIT = 50;

export const EMPTY_COUNTS: OutboxCounts = {
  pending: 0,
  inflight: 0,
  uncertain: 0,
  done: 0,
  failed: 0,
  cancelled: 0,
};

interface OutboxUi {
  open: boolean;
  operations: OutboxOp[];
  counts: OutboxCounts;
  total: number;
  nextCursor: string | null;
  loading: boolean;
  error: string | null;
  /**
   * The duplicate-risk acknowledgement is stated per operation and cleared with
   * the panel: an uncertain send is never retried on a timer or on a stale
   * checkbox the user cannot see (P6.6).
   */
  acknowledged: Record<number, boolean>;
  setOpen: (open: boolean) => void;
  refresh: (accountIds: string[]) => Promise<void>;
  loadMore: (accountIds: string[]) => Promise<void>;
  setAcknowledged: (opId: number, value: boolean) => void;
  retry: (opId: number, acknowledgeDuplicateRisk: boolean) => Promise<boolean>;
  applyCounts: (counts: OutboxCounts) => void;
}

export const useOutbox = create<OutboxUi>((set, get) => ({
  open: false,
  operations: [],
  counts: EMPTY_COUNTS,
  total: 0,
  nextCursor: null,
  loading: false,
  error: null,
  acknowledged: {},
  setOpen: (open) => set({ open, ...(open ? {} : { acknowledged: {} }) }),
  refresh: async (accountIds) => {
    if (!accountIds.length) {
      set({ operations: [], total: 0, nextCursor: null, counts: EMPTY_COUNTS, error: null });
      return;
    }
    set({ loading: true });
    try {
      const page = await api.outbox_list({
        accountIds,
        states: OUTBOX_VISIBLE_STATES,
        cursor: null,
        limit: OUTBOX_PAGE_LIMIT,
      });
      set({
        operations: page.operations,
        nextCursor: page.nextCursor,
        total: page.total,
        counts: page.counts,
        loading: false,
        error: null,
      });
    } catch (e) {
      set({ loading: false, error: readSiftError(e).message });
    }
  },
  loadMore: async (accountIds) => {
    const cursor = get().nextCursor;
    if (!cursor || get().loading) return;
    set({ loading: true });
    try {
      const page = await api.outbox_list({
        accountIds,
        states: OUTBOX_VISIBLE_STATES,
        cursor,
        limit: OUTBOX_PAGE_LIMIT,
      });
      set((st) => ({
        operations: [...st.operations, ...page.operations],
        nextCursor: page.nextCursor,
        total: page.total,
        counts: page.counts,
        loading: false,
      }));
    } catch (e) {
      set({ loading: false, error: readSiftError(e).message });
    }
  },
  setAcknowledged: (opId, value) => set((st) => ({ acknowledged: { ...st.acknowledged, [opId]: value } })),
  retry: async (opId, acknowledgeDuplicateRisk) => {
    try {
      const updated = await api.outbox_retry({ opId, acknowledgeDuplicateRisk });
      set((st) => ({
        operations: st.operations.map((o) => (o.opId === opId ? updated : o)),
        error: null,
      }));
      return true;
    } catch (e) {
      set({ error: readSiftError(e).message });
      return false;
    }
  },
  applyCounts: (counts) => set({ counts }),
}));
