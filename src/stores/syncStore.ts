import { create } from 'zustand';
import type { SyncStatus } from '../app/ipc/types';

interface Sync {
  byAccount: Record<string, SyncStatus>;
  online: boolean;
  pending: Record<string, number>;
  setStatus: (s: SyncStatus) => void;
  setOnline: (v: boolean) => void;
  setPending: (account: string, n: number) => void;
}

export const useSync = create<Sync>((set) => ({
  byAccount: {},
  online: true,
  pending: {},
  setStatus: (s) => set((st) => ({ byAccount: { ...st.byAccount, [s.account_id]: s } })),
  setOnline: (online) => set({ online }),
  setPending: (account, n) => set((st) => ({ pending: { ...st.pending, [account]: n } })),
}));
