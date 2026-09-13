import { create } from 'zustand';

/**
 * Selection is stored by account/thread key (`accountId:threadId`) and never by
 * index, so inserts/removals above the window cannot silently retarget an
 * action. `focusedKey` is the active list row; the renderer derives its index.
 */
interface Sel {
  focusedKey: string | null;
  selectedIds: Set<string>;
  anchorKey: string | null;
  setFocus: (key: string | null) => void;
  move: (delta: number, keys: string[]) => void;
  toggle: (key: string) => void;
  extendTo: (key: string, keys: string[]) => void;
  selectAll: (keys: string[]) => void;
  clearSelection: () => void;
  clearKeepFocus: () => void;
}

export const useSelection = create<Sel>((set, get) => ({
  focusedKey: null,
  selectedIds: new Set(),
  anchorKey: null,
  setFocus: (focusedKey) => set({ focusedKey }),
  move: (delta, keys) => {
    if (!keys.length) return;
    const { focusedKey } = get();
    const at = focusedKey ? keys.indexOf(focusedKey) : -1;
    const base = at < 0 ? (delta > 0 ? -1 : 0) : at;
    const next = keys[Math.max(0, Math.min(keys.length - 1, base + delta))];
    if (next !== undefined) set({ focusedKey: next });
  },
  toggle: (key) => {
    const s = new Set(get().selectedIds);
    if (s.has(key)) s.delete(key);
    else s.add(key);
    set({ selectedIds: s, anchorKey: key });
  },
  extendTo: (key, keys) => {
    const { anchorKey, focusedKey } = get();
    const anchor = anchorKey ?? focusedKey;
    const s = new Set<string>();
    if (!anchor) {
      s.add(key);
    } else {
      const a = keys.indexOf(anchor);
      const b = keys.indexOf(key);
      if (a < 0 || b < 0) {
        s.add(key);
      } else {
        const [from, to] = a < b ? [a, b] : [b, a];
        for (let i = from; i <= to; i++) {
          const k = keys[i];
          if (k !== undefined) s.add(k);
        }
      }
    }
    set({ selectedIds: s, anchorKey: anchor ?? key, focusedKey: key });
  },
  selectAll: (keys) => set({ selectedIds: new Set(keys), anchorKey: keys[0] ?? null }),
  clearSelection: () => set({ selectedIds: new Set(), anchorKey: null }),
  clearKeepFocus: () => set({ selectedIds: new Set(), anchorKey: null }),
}));
