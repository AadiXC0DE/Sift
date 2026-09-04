import { create } from 'zustand';
import type { View } from '../app/ipc/types';

export type PaneLayout = 'right' | 'bottom' | 'off';

interface ViewState {
  accountScope: string | 'all';
  view: View;
  threadId: string | null;
  paneLayout: PaneLayout;
  prevView?: { view: View; scroll: number; cursor: number };
  setView: (v: View) => void;
  setScope: (s: string | 'all') => void;
  setThread: (id: string | null) => void;
  cyclePane: () => void;
  setPane: (p: PaneLayout) => void;
  stashForSearch: (scroll: number, cursor: number) => void;
  restore: () => void;
}

const order: PaneLayout[] = ['right', 'bottom', 'off'];

export const useView = create<ViewState>((set, get) => ({
  accountScope: 'all',
  view: { kind: 'inbox' },
  threadId: null,
  paneLayout: 'right',
  setView: (view) => set({ view, threadId: null }),
  setScope: (accountScope) => set({ accountScope }),
  setThread: (threadId) => set({ threadId }),
  cyclePane: () => {
    const cur = get().paneLayout;
    set({ paneLayout: order[(order.indexOf(cur) + 1) % order.length] });
  },
  setPane: (paneLayout) => set({ paneLayout }),
  stashForSearch: (scroll, cursor) => set((s) => ({ prevView: { view: s.view, scroll, cursor } })),
  restore: () => {
    const p = get().prevView;
    if (p) set({ view: p.view, prevView: undefined });
  },
}));

export function viewKey(view: View, scope: string | 'all'): string {
  return `${scope}:${JSON.stringify(view)}`;
}
