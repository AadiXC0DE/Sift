import { describe, expect, it } from 'vitest';
import type { ThreadRow } from '../../app/ipc/types';
import { MAX_WINDOW, PAGE_SIZE, appendPage, capWindow, dedupeRows, rowKey } from './threadWindow';

function row(accountId: string, id: string): ThreadRow {
  return {
    accountId,
    id,
    subject: id,
    snippet: '',
    participants: [],
    lastMessageAt: 0,
    messageCount: 1,
    unreadCount: 0,
    isStarred: false,
    hasAttachments: false,
    labelIds: [],
  };
}

describe('P3.3 window merge', () => {
  it('keys rows by account and thread, dropping duplicates in server order', () => {
    expect(rowKey(row('a', 't1'))).toBe('a:t1');
    const merged = dedupeRows([row('a', 't1'), row('b', 't1'), row('a', 't1')]);
    expect(merged.map(rowKey)).toEqual(['a:t1', 'b:t1']);
  });

  it('append preserves server order and never duplicates a loaded row', () => {
    const loaded = [row('a', 't1'), row('a', 't2')];
    const page = [row('a', 't2'), row('a', 't3')];
    expect(appendPage(loaded, page).map((r) => r.id)).toEqual(['t1', 't2', 't3']);
  });
});

describe('P3.4 bounded window', () => {
  it('leaves a window below the cap untouched', () => {
    const rows = Array.from({ length: 400 }, (_, i) => row('a', `t${i}`));
    expect(capWindow(rows).evicted).toBe(0);
    expect(capWindow(rows).rows).toHaveLength(400);
  });

  it('evicts whole loaded pages from the head at the cap', () => {
    const rows = Array.from({ length: 1100 }, (_, i) => row('a', `t${i}`));
    const capped = capWindow(rows);
    expect(capped.evicted).toBe(PAGE_SIZE);
    expect(capped.rows).toHaveLength(1000);
    expect(capped.rows[0].id).toBe('t100');
  });

  it('paging through 100k messages never grows past the cap', () => {
    let rows: ThreadRow[] = [];
    let cursor = 0;
    for (let page = 0; page < 1000; page++) {
      const next = Array.from({ length: PAGE_SIZE }, (_, i) => row('a', `t${cursor + i}`));
      cursor += PAGE_SIZE;
      rows = capWindow(appendPage(rows, next)).rows;
      expect(rows.length).toBeLessThanOrEqual(MAX_WINDOW);
    }
    expect(rows).toHaveLength(MAX_WINDOW);
    // The tail position survives eviction: the newest eviction kept the last
    // 1000 rows, so the window ends at the last fetched message.
    expect(rows[rows.length - 1].id).toBe(`t${cursor - 1}`);
  });
});
