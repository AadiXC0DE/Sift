/**
 * Reading/list pane geometry (P9.5).
 *
 * The divider's two sizes are per-machine layout state, not account
 * configuration, so they live in versioned local storage rather than the
 * settings table; every read re-clamps, so a stale or hand-edited value can
 * never produce an unusable window.
 */

export const LIST_MIN_W = 280;
/** Matches the historical `maxWidth` of the list column. */
export const LIST_MAX_W = 560;
export const READER_MIN_W = 320;

export const LIST_MIN_H = 140;
export const READER_MIN_H = 180;

/** The divider itself; both panes have to fit *plus* the handle. */
export const DIVIDER_SIZE = 6;

const STORAGE_KEY = 'sift.ui.pane-sizes.v1';

export interface PaneSizes {
  listWidth: number;
  listHeight: number;
}

export const DEFAULT_PANE_SIZES: PaneSizes = { listWidth: 380, listHeight: 260 };

/** Keep the list wide enough to be readable and leave the reader its minimum. */
export function clampListWidth(width: number, available: number): number {
  const ceiling = Math.max(LIST_MIN_W, Math.min(LIST_MAX_W, available - READER_MIN_W - DIVIDER_SIZE));
  return Math.round(Math.min(ceiling, Math.max(LIST_MIN_W, width)));
}

export function clampListHeight(height: number, available: number): number {
  const ceiling = Math.max(LIST_MIN_H, available - READER_MIN_H - DIVIDER_SIZE);
  return Math.round(Math.min(ceiling, Math.max(LIST_MIN_H, height)));
}

/** True when a side-by-side layout leaves both panes usable. */
export function panesFitSideBySide(available: number): boolean {
  return available >= LIST_MIN_W + READER_MIN_W + DIVIDER_SIZE;
}

/** True when a stacked layout leaves both panes usable. */
export function panesFitStacked(available: number): boolean {
  return available >= LIST_MIN_H + READER_MIN_H + DIVIDER_SIZE;
}

export function readPaneSizes(storage: Storage | undefined = safeStorage()): PaneSizes {
  if (!storage) return { ...DEFAULT_PANE_SIZES };
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_PANE_SIZES };
    const parsed = JSON.parse(raw) as Partial<PaneSizes>;
    return {
      // Clamped against a window that can hold the largest legal list, so a
      // stored value never exceeds the documented maximum; the live window
      // clamps again on every render.
      listWidth: clampListWidth(
        Number(parsed.listWidth ?? DEFAULT_PANE_SIZES.listWidth),
        LIST_MAX_W + READER_MIN_W + DIVIDER_SIZE,
      ),
      listHeight: clampListHeight(Number(parsed.listHeight ?? DEFAULT_PANE_SIZES.listHeight), 10_000),
    };
  } catch {
    return { ...DEFAULT_PANE_SIZES };
  }
}

export function writePaneSizes(sizes: PaneSizes, storage: Storage | undefined = safeStorage()): void {
  if (!storage) return;
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(sizes));
  } catch {
    /* private mode or quota: the layout simply does not persist */
  }
}

function safeStorage(): Storage | undefined {
  try {
    return window.localStorage;
  } catch {
    return undefined;
  }
}
