import { create } from 'zustand';
import type { ConnectivityState, SyncStatus } from '../app/ipc/types';

export interface PendingItem {
  label: string;
  count: number;
}

interface Sync {
  byAccount: Record<string, SyncStatus>;
  /**
   * Native connectivity per account (P4.6). `navigator.onLine` is only a hint
   * that the host has a network; this is the outcome of real provider
   * operations, so the UI reports what actually happened to each account.
   */
  connectivity: Record<string, ConnectivityState>;
  pending: Record<string, number>;
  pendingSummary: Record<string, PendingItem[]>;
  setStatus: (s: SyncStatus) => void;
  /** One account's state changed (the `connectivity:state` event). */
  setConnectivity: (s: ConnectivityState) => void;
  /** The startup snapshot: replaces the map so removed accounts disappear. */
  setConnectivityAll: (list: ConnectivityState[]) => void;
  setPending: (account: string, n: number, summary?: PendingItem[]) => void;
}

export const useSync = create<Sync>((set) => ({
  byAccount: {},
  connectivity: {},
  pending: {},
  pendingSummary: {},
  setStatus: (s) => set((st) => ({ byAccount: { ...st.byAccount, [s.account_id]: s } })),
  setConnectivity: (s) => set((st) => ({ connectivity: { ...st.connectivity, [s.accountId]: s } })),
  setConnectivityAll: (list) =>
    set(() => {
      const connectivity: Record<string, ConnectivityState> = {};
      for (const s of list) connectivity[s.accountId] = s;
      return { connectivity };
    }),
  setPending: (account, n, summary) =>
    set((st) => ({
      pending: { ...st.pending, [account]: n },
      pendingSummary: summary ? { ...st.pendingSummary, [account]: summary } : st.pendingSummary,
    })),
}));
