import { create } from 'zustand';
import type { ConnectivityState, SyncStatus } from '../app/ipc/types';

export interface PendingItem {
  label: string;
  count: number;
}

/**
 * One account's outbox counts as the backend reports them (P6.6). The panel and
 * the sidebar read these instead of deriving "failed" from a pending number:
 * the old footer reported failed work as zero because it only ever saw a single
 * pending count, so a stuck operation looked like it was still moving.
 */
export interface OutboxAccountState {
  pending: number;
  inflight: number;
  failed: number;
  uncertain: number;
  summary: PendingItem[];
}

const EMPTY_OUTBOX: OutboxAccountState = {
  pending: 0,
  inflight: 0,
  failed: 0,
  uncertain: 0,
  summary: [],
};

interface Sync {
  byAccount: Record<string, SyncStatus>;
  /**
   * Native connectivity per account (P4.6). `navigator.onLine` is only a hint
   * that the host has a network; this is the outcome of real provider
   * operations, so the UI reports what actually happened to each account.
   */
  connectivity: Record<string, ConnectivityState>;
  outbox: Record<string, OutboxAccountState>;
  setStatus: (s: SyncStatus) => void;
  /** One account's state changed (the `connectivity:state` event). */
  setConnectivity: (s: ConnectivityState) => void;
  /** The startup snapshot: replaces the map so removed accounts disappear. */
  setConnectivityAll: (list: ConnectivityState[]) => void;
  setOutbox: (account: string, state: OutboxAccountState) => void;
  /** Drop an account's counts when it is removed. */
  clearOutboxAccount: (account: string) => void;
}

export const useSync = create<Sync>((set) => ({
  byAccount: {},
  connectivity: {},
  outbox: {},
  setStatus: (s) => set((st) => ({ byAccount: { ...st.byAccount, [s.account_id]: s } })),
  setConnectivity: (s) => set((st) => ({ connectivity: { ...st.connectivity, [s.accountId]: s } })),
  setConnectivityAll: (list) =>
    set(() => {
      const connectivity: Record<string, ConnectivityState> = {};
      for (const s of list) connectivity[s.accountId] = s;
      return { connectivity };
    }),
  setOutbox: (account, state) => set((st) => ({ outbox: { ...st.outbox, [account]: state } })),
  clearOutboxAccount: (account) =>
    set((st) => {
      const next = { ...st.outbox };
      delete next[account];
      return { outbox: next };
    }),
}));

export function outboxTotals(outbox: Record<string, OutboxAccountState>): OutboxAccountState {
  const total: OutboxAccountState = { ...EMPTY_OUTBOX, summary: [] };
  for (const s of Object.values(outbox)) {
    total.pending += s.pending;
    total.inflight += s.inflight;
    total.failed += s.failed;
    total.uncertain += s.uncertain;
    total.summary.push(...s.summary);
  }
  return total;
}
