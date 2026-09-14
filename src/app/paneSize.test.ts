import { describe, expect, it } from 'vitest';
import {
  DEFAULT_PANE_SIZES,
  LIST_MAX_W,
  LIST_MIN_H,
  LIST_MIN_W,
  READER_MIN_H,
  READER_MIN_W,
  clampListHeight,
  clampListWidth,
  panesFitSideBySide,
  panesFitStacked,
  readPaneSizes,
  writePaneSizes,
} from './paneSize';

function memoryStorage(initial?: string): Storage {
  const store = new Map<string, string>();
  if (initial !== undefined) store.set('sift.ui.pane-sizes.v1', initial);
  return {
    get length() {
      return store.size;
    },
    clear: () => store.clear(),
    getItem: (k: string) => store.get(k) ?? null,
    key: (i: number) => [...store.keys()][i] ?? null,
    removeItem: (k: string) => void store.delete(k),
    setItem: (k: string, v: string) => void store.set(k, v),
  } as Storage;
}

describe('P9.5 pane geometry', () => {
  it('keeps a usable minimum for both panes', () => {
    expect(clampListWidth(10, 1000)).toBe(LIST_MIN_W);
    expect(clampListWidth(10_000, 1000)).toBe(LIST_MAX_W);
    // The reader's minimum wins over the user's remembered width.
    expect(clampListWidth(900, 640)).toBe(640 - READER_MIN_W - 6);
    expect(clampListHeight(1, 1000)).toBe(LIST_MIN_H);
    expect(clampListHeight(10_000, 1000)).toBe(1000 - READER_MIN_H - 6);
  });

  it('reports when the window is too small for two panes', () => {
    // 720x480 with a 220px sidebar leaves ~500px for the columns: side by side
    // is impossible, so the app has to fall back to pane-off.
    expect(panesFitSideBySide(500)).toBe(false);
    expect(panesFitSideBySide(LIST_MIN_W + READER_MIN_W + 6)).toBe(true);
    expect(panesFitStacked(480 - 30 - 26)).toBe(true);
    expect(panesFitStacked(200)).toBe(false);
  });

  it('falls back to the defaults when nothing is stored', () => {
    expect(readPaneSizes(memoryStorage())).toEqual(DEFAULT_PANE_SIZES);
  });

  it('re-clamps a stored value that no longer fits', () => {
    const storage = memoryStorage(JSON.stringify({ listWidth: 99_999, listHeight: -40 }));
    const sizes = readPaneSizes(storage);
    expect(sizes.listWidth).toBe(LIST_MAX_W);
    expect(sizes.listHeight).toBe(LIST_MIN_H);
  });

  it('survives corrupt or unavailable storage', () => {
    expect(readPaneSizes(memoryStorage('not json'))).toEqual(DEFAULT_PANE_SIZES);
    expect(readPaneSizes(undefined)).toEqual(DEFAULT_PANE_SIZES);
    expect(() => writePaneSizes(DEFAULT_PANE_SIZES, undefined)).not.toThrow();
    const throwing = {
      ...memoryStorage(),
      getItem: () => {
        throw new Error('denied');
      },
    } as Storage;
    expect(readPaneSizes(throwing)).toEqual(DEFAULT_PANE_SIZES);
  });

  it('round-trips the sizes', () => {
    const storage = memoryStorage();
    writePaneSizes({ listWidth: 300, listHeight: 200 }, storage);
    expect(readPaneSizes(storage)).toEqual({ listWidth: 300, listHeight: 200 });
  });
});
