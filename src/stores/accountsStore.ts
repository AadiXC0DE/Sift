import { create } from 'zustand';
import type { Account } from '../app/ipc/types';
import { api } from '../app/ipc/commands';

interface Acc {
  accounts: Account[];
  included: Record<string, boolean>;
  loading: boolean;
  refresh: () => Promise<void>;
  includedIds: () => string[];
  remove: (id: string) => Promise<void>;
}

export const useAccounts = create<Acc>((set, get) => ({
  accounts: [],
  included: {},
  loading: false,
  refresh: async () => {
    set({ loading: true });
    try {
      const accounts = await api.accounts_list();
      const included: Record<string, boolean> = {};
      for (const a of accounts) included[a.id] = get().included[a.id] ?? true;
      set({ accounts, included });
    } catch {
      /* offline */
    }
    set({ loading: false });
  },
  includedIds: () =>
    get()
      .accounts.filter((a) => get().included[a.id] !== false)
      .map((a) => a.id),
  remove: async (id) => {
    await api.accounts_remove(id);
    await get().refresh();
  },
}));
