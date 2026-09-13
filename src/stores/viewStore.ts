import { create } from 'zustand';
import type { ThreadRef, View } from '../app/ipc/types';

export type PaneLayout = 'right' | 'bottom' | 'off';

interface ViewState {
  accountScope: string | 'all';
  view: View;
  /**
   * The open conversation, qualified by account (P3.2). Provider thread IDs are
   * only unique within an account, so a bare id cannot identify a row.
   */
  openThread: ThreadRef | null;
  paneLayout: PaneLayout;
  setView: (v: View) => void;
  setScope: (s: string | 'all') => void;
  setOpenThread: (ref: ThreadRef | null) => void;
  cyclePane: () => void;
  setPane: (p: PaneLayout) => void;
}

const order: PaneLayout[] = ['right', 'bottom', 'off'];

export const useView = create<ViewState>((set, get) => ({
  accountScope: 'all',
  view: { kind: 'inbox' },
  openThread: null,
  paneLayout: 'right',
  setView: (view) => set({ view, openThread: null }),
  // Narrowing the scope clears a reader that belongs to another account; the
  // list's auto-open effect restores a thread inside the new scope.
  setScope: (accountScope) =>
    set((s) => ({
      accountScope,
      openThread:
        s.openThread && accountScope !== 'all' && s.openThread.accountId !== accountScope
          ? null
          : s.openThread,
    })),
  setOpenThread: (openThread) => set({ openThread }),
  cyclePane: () => {
    const cur = get().paneLayout;
    set({ paneLayout: order[(order.indexOf(cur) + 1) % order.length] });
  },
  setPane: (paneLayout) => set({ paneLayout }),
}));

export function viewKey(view: View, scope: string | 'all'): string {
  return `${scope}:${JSON.stringify(view)}`;
}
