import { create } from 'zustand';
import type { SyncStatus } from '../app/ipc/types';

export interface PendingItem {
  label: string;
  count: number;
}

interface Sync {
  byAccount: Record<string, SyncStatus>;
  online: boolean;
  pending: Record<string, number>;
  pendingSummary: Record<string, PendingItem[]>;
  setStatus: (s: SyncStatus) => void;
  setOnline: (v: boolean) => void;
  setPending: (account: string, n: number, summary?: PendingItem[]) => void;
}

export const useSync = create<Sync>((set) => ({
  byAccount: {},
  online: true,
  pending: {},
  pendingSummary: {},
  setStatus: (s) => set((st) => ({ byAccount: { ...st.byAccount, [s.account_id]: s } })),
  setOnline: (online) => set({ online }),
  setPending: (account, n, summary) =>
    set((st) => ({
      pending: { ...st.pending, [account]: n },
      pendingSummary: summary ? { ...st.pendingSummary, [account]: summary } : st.pendingSummary,
    })),
}));
