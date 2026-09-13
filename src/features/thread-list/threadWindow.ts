import type { ThreadRow } from '../../app/ipc/types';

export const PAGE_SIZE = 100;
export const MAX_WINDOW = 1000;
// Store events are coalesced inside the 16–50 ms window the spec allows.
export const COALESCE_MS = 32;

export function rowKey(row: Pick<ThreadRow, 'accountId' | 'id'>): string {
  return `${row.accountId}:${row.id}`;
}

/** Drop duplicate account/thread keys, keeping the first server occurrence. */
export function dedupeRows(rows: ThreadRow[]): ThreadRow[] {
  const seen = new Set<string>();
  const out: ThreadRow[] = [];
  for (const row of rows) {
    const key = rowKey(row);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(row);
  }
  return out;
}

/** Append a page, preserving server order and never duplicating a loaded row. */
export function appendPage(current: ThreadRow[], page: ThreadRow[]): ThreadRow[] {
  const seen = new Set(current.map(rowKey));
  const out = current.slice();
  for (const row of page) {
    const key = rowKey(row);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(row);
  }
  return out;
}

/**
 * Bound the loaded window. Whole pages are evicted from the head, which keeps
 * the *tail* position (nextCursor) valid for the next append.
 */
export function capWindow(
  rows: ThreadRow[],
  cap = MAX_WINDOW,
  page = PAGE_SIZE,
): { rows: ThreadRow[]; evicted: number } {
  if (rows.length <= cap) return { rows, evicted: 0 };
  const excess = rows.length - cap;
  let evicted = Math.ceil(excess / page) * page;
  const maxEvictable = Math.floor(rows.length / page) * page;
  evicted = Math.min(evicted, maxEvictable);
  if (evicted <= 0) return { rows, evicted: 0 };
  return { rows: rows.slice(evicted), evicted };
}
