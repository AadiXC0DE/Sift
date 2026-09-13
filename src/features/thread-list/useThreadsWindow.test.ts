import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ThreadRow, ThreadsPage, ThreadsQuery, View } from '../../app/ipc/types';
import { useAccounts } from '../../stores/accountsStore';
import { useSelection } from '../../stores/selectionStore';
import { useView } from '../../stores/viewStore';
import { COALESCE_MS } from './threadWindow';
import { useThreadsWindow } from './useThreadsWindow';

const threadsQuery = vi.fn<(q: ThreadsQuery) => Promise<ThreadsPage>>();

vi.mock('../../app/ipc/commands', () => ({
  api: {
    threads_query: (q: ThreadsQuery) => threadsQuery(q),
  },
}));

type StoreHandler = (payload: unknown) => void;
const listeners = new Map<string, Set<StoreHandler>>();

vi.mock('../../app/ipc/events', () => ({
  on: (event: string, handler: StoreHandler) => {
    const set = listeners.get(event) ?? new Set<StoreHandler>();
    set.add(handler);
    listeners.set(event, set);
    return Promise.resolve(() => set.delete(handler));
  },
}));

function emit(event: string, payload: unknown) {
  for (const handler of listeners.get(event) ?? []) handler(payload);
}

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
    labelIds: ['INBOX'],
  };
}

function makeAccount(id: string) {
  return {
    id,
    email: `${id}@x`,
    provider: 'gmail',
    color: 'blue',
    sync_state: 'idle',
    created_at: 0,
    sort_order: 0,
  } as never;
}

function page(rows: ThreadRow[], nextCursor?: string): ThreadsPage {
  return { rows, nextCursor, generation: 1 };
}

/** Cursor format is `c<offset>`; rows are addressed by their index. */
function cursorOffset(cursor?: string): number {
  return cursor ? Number(cursor.slice(1)) : 0;
}

function server(all: () => ThreadRow[]) {
  return (q: ThreadsQuery): Promise<ThreadsPage> => {
    const rows = all();
    const start = cursorOffset(q.cursor);
    const slice = rows.slice(start, start + q.limit);
    const next = start + q.limit < rows.length ? `c${start + q.limit}` : undefined;
    return Promise.resolve(page(slice, next));
  };
}

function rowsFor(accountId: string, count: number, offset = 0): ThreadRow[] {
  return Array.from({ length: count }, (_, i) => row(accountId, `t${offset + i}`));
}

const VIEW: View = { kind: 'inbox' };

// `Promise.withResolvers` needs ES2024 lib; this project targets ES2022.
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

async function tick(times = 6) {
  for (let i = 0; i < times; i++) await act(async () => void (await Promise.resolve()));
}

/** Advance beyond the coalescing window, then drain the resulting promises. */
async function advance(ms = COALESCE_MS + 1) {
  await act(async () => {
    vi.advanceTimersByTime(ms);
  });
  await tick();
}

async function until(condition: () => boolean, label: string) {
  for (let i = 0; i < 40; i++) {
    if (condition()) return;
    await advance(COALESCE_MS);
  }
  throw new Error(`timed out waiting for ${label}`);
}

function render() {
  useAccounts.setState({
    accounts: [makeAccount('a'), makeAccount('b')] as never,
    included: { a: true },
    loading: false,
  });
  return renderHook(({ view }: { view: View }) => useThreadsWindow(view), { initialProps: { view: VIEW } });
}

beforeEach(() => {
  vi.useFakeTimers();
  threadsQuery.mockReset();
  listeners.clear();
  useView.setState({ accountScope: 'a', openThread: null });
  useSelection.setState({ focusedKey: null, selectedIds: new Set(), anchorKey: null });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('P3.3 deterministic loads and pagination', () => {
  it('admits one request per cursor even when the sentinel is seen twice', async () => {
    const all = rowsFor('a', 250);
    const gate = deferred<ThreadsPage>();
    threadsQuery.mockImplementationOnce(server(() => all)).mockImplementationOnce(() => gate.promise);
    const { result } = render();
    await until(() => result.current.rows.length === 100, 'initial page');

    act(() => {
      result.current.loadMore();
      result.current.loadMore();
    });
    expect(threadsQuery).toHaveBeenCalledTimes(2);
    await act(async () => {
      gate.resolve(page(rowsFor('a', 100, 100), 'c200'));
    });
    await tick();
    expect(result.current.rows).toHaveLength(200);
  });

  it('changes nothing when a stale append resolves after an account switch', async () => {
    const all = rowsFor('a', 250);
    const gate = deferred<ThreadsPage>();
    const bRows = [row('b', 'b1'), row('b', 'b2')];
    threadsQuery
      .mockImplementationOnce(server(() => all))
      .mockImplementationOnce(() => gate.promise)
      .mockImplementationOnce(() => Promise.resolve(page(bRows)));
    const { result } = render();
    await until(() => result.current.rows.length === 100, 'initial page');

    act(() => {
      result.current.loadMore();
    });
    await act(async () => {
      useView.setState({ accountScope: 'b' });
    });
    await until(() => result.current.rows.length === 2, 'switched window');

    await act(async () => {
      gate.resolve(page(rowsFor('a', 100, 100), 'c200'));
    });
    await tick();
    expect(result.current.rows.map((r) => r.id)).toEqual(['b1', 'b2']);
  });

  it('surfaces a retryable query error instead of an empty-inbox claim', async () => {
    threadsQuery.mockRejectedValueOnce(new Error('imap unavailable'));
    const { result } = render();
    await until(() => result.current.error !== null, 'error state');
    expect(result.current.error).toBe('imap unavailable');
    expect(result.current.rows).toEqual([]);
    expect(result.current.initialLoading).toBe(false);

    threadsQuery.mockImplementationOnce(server(() => rowsFor('a', 3)));
    act(() => result.current.reload());
    await until(() => result.current.rows.length === 3, 'retry result');
    expect(result.current.error).toBeNull();
  });

  it('renders only the final view after ten rapid view changes', async () => {
    threadsQuery.mockImplementation((q) =>
      Promise.resolve(page([row(q.accountIds[0] ?? 'a', JSON.stringify(q.view))])),
    );
    const views: View[] = Array.from({ length: 10 }, (_, i) => ({ kind: 'label', labelId: `L${i}` }));
    const { result, rerender } = renderHook(({ view }: { view: View }) => useThreadsWindow(view), {
      initialProps: { view: VIEW as View },
    });
    for (const v of views) rerender({ view: v });
    const finalView = views[views.length - 1];
    await until(() => result.current.rows.length === 1, 'final view rows');
    expect(result.current.rows[0].id).toBe(JSON.stringify(finalView));
    expect(threadsQuery.mock.calls[threadsQuery.mock.calls.length - 1][0].view).toEqual(finalView);
  });

  it('requeries with the right account ids when inclusion changes at the same count', async () => {
    useView.setState({ accountScope: 'all' });
    useAccounts.setState({
      accounts: [makeAccount('a'), makeAccount('b')] as never,
      included: { a: true, b: true },
      loading: false,
    });
    threadsQuery.mockImplementation(server(() => rowsFor('a', 2)));
    const { result } = renderHook(({ view }: { view: View }) => useThreadsWindow(view), {
      initialProps: { view: VIEW },
    });
    await until(() => result.current.rows.length === 2, 'initial unified page');
    expect(threadsQuery.mock.calls[0][0].accountIds).toEqual(['a', 'b']);

    await act(async () => {
      useAccounts.setState({ included: { a: false, b: true } });
    });
    await until(() => threadsQuery.mock.calls.length > 1, 'requery');
    expect(threadsQuery.mock.calls[threadsQuery.mock.calls.length - 1][0].accountIds).toEqual(['b']);
  });
});

describe('P3.4 refresh without collapsing the window', () => {
  it('shows a new newest thread when exactly one page is loaded', async () => {
    let all = rowsFor('a', 250);
    threadsQuery.mockImplementation(server(() => all));
    const { result } = render();
    await until(() => result.current.rows.length === 100, 'initial page');

    all = [row('a', 'newest'), ...all];
    await act(async () => {
      emit('store:threads', { account_id: 'a', thread_ids: ['newest'] });
    });
    await advance();
    await until(() => result.current.rows[0]?.id === 'newest', 'newest thread');
    expect(result.current.rows).toHaveLength(100);
    expect(threadsQuery.mock.calls[threadsQuery.mock.calls.length - 1][0].limit).toBe(100);
  });

  it('keeps 400 loaded rows and the top position when an archive arrives', async () => {
    let all = rowsFor('a', 1000);
    threadsQuery.mockImplementation(server(() => all));
    const { result } = render();
    await until(() => result.current.rows.length === 100, 'initial page');
    for (let i = 0; i < 3; i++) {
      act(() => result.current.loadMore());
      await until(() => result.current.rows.length === (i + 2) * 100, `page ${i + 2}`);
    }

    all = all.filter((r) => r.id !== 't50');
    await act(async () => {
      emit('store:threads', { account_id: 'a', thread_ids: ['t50'] });
    });
    await advance();
    await until(() => !result.current.rows.some((r) => r.id === 't50'), 'archived row removed');
    expect(result.current.rows[0].id).toBe('t0');
    expect(result.current.rows).toHaveLength(400);
    expect(result.current.refreshRevision).toBeGreaterThan(0);
  });

  it('ignores events from accounts outside the query', async () => {
    threadsQuery.mockImplementation(server(() => rowsFor('a', 5)));
    const { result } = render();
    await until(() => result.current.rows.length === 5, 'initial page');
    const calls = threadsQuery.mock.calls.length;

    await act(async () => {
      emit('store:threads', { account_id: 'b', thread_ids: ['t9'] });
    });
    await advance();
    expect(threadsQuery.mock.calls.length).toBe(calls);
  });

  it('keeps receiving events after an equal-length replacement', async () => {
    const all = rowsFor('a', 5);
    threadsQuery.mockImplementation(server(() => all));
    const { result } = render();
    await until(() => result.current.rows.length === 5, 'initial page');

    await act(async () => {
      emit('store:threads', { account_id: 'a', thread_ids: ['t1'] });
    });
    await advance();
    const afterFirst = threadsQuery.mock.calls.length;
    expect(afterFirst).toBeGreaterThan(1);

    await act(async () => {
      emit('store:threads', { account_id: 'a', thread_ids: ['t1'] });
    });
    await advance();
    expect(threadsQuery.mock.calls.length).toBeGreaterThan(afterFirst);
  });

  it('never grows the loaded window past 1000 rows while paging', async () => {
    threadsQuery.mockImplementation(server(() => rowsFor('a', 100_000)));
    const { result } = render();
    await until(() => result.current.rows.length === 100, 'initial page');
    for (let pageIndex = 0; pageIndex < 12; pageIndex++) {
      act(() => result.current.loadMore());
      await until(() => !result.current.loadingMore, `append ${pageIndex}`);
    }
    expect(result.current.rows).toHaveLength(1000);
  });
});
