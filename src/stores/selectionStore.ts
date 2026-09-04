import { create } from 'zustand';

interface Sel {
  focusedIndex: number;
  selectedIds: Set<string>;
  anchorIndex: number | null;
  setFocus: (i: number) => void;
  move: (d: number, count: number) => void;
  toggle: (id: string) => void;
  extendTo: (index: number, idAt: (i: number) => string | undefined) => void;
  selectAll: (ids: string[]) => void;
  clearSelection: () => void;
  clearKeepFocus: () => void;
}

export const useSelection = create<Sel>((set, get) => ({
  focusedIndex: 0,
  selectedIds: new Set(),
  anchorIndex: null,
  setFocus: (focusedIndex) => set({ focusedIndex }),
  move: (d, count) => {
    const { focusedIndex } = get();
    const n = Math.max(0, Math.min(count - 1, focusedIndex + d));
    set({ focusedIndex: n });
  },
  toggle: (id) => {
    const s = new Set(get().selectedIds);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    set({ selectedIds: s, anchorIndex: get().focusedIndex });
  },
  extendTo: (index, idAt) => {
    const { anchorIndex, focusedIndex } = get();
    const anchor = anchorIndex ?? focusedIndex;
    const s = new Set<string>();
    const [a, b] = anchor < index ? [anchor, index] : [index, anchor];
    for (let i = a; i <= b; i++) {
      const id = idAt(i);
      if (id) s.add(id);
    }
    set({ selectedIds: s, focusedIndex: index });
  },
  selectAll: (ids) => set({ selectedIds: new Set(ids) }),
  clearSelection: () => set({ selectedIds: new Set(), anchorIndex: null }),
  clearKeepFocus: () => set({ selectedIds: new Set(), anchorIndex: null }),
}));
