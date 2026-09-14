import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Draft, DraftPage } from '../../app/ipc/types';

interface ListArgs {
  accountIds: string[];
  cursor?: string;
  limit?: number;
}

const draftsList = vi.fn<(args: ListArgs) => Promise<DraftPage>>();

vi.mock('../../app/ipc/commands', () => ({
  api: {
    drafts_list: (args: ListArgs) => draftsList(args),
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

import { useDrafts } from './useDrafts';

/**
 * A draft carrying the full persisted contract. The cast is required because
 * `state`/`localId` land with the drafts_list command; the fixture already
 * matches the shape the hook reads.
 */
function makeDraft(localId: string, over: Record<string, unknown> = {}): Draft {
  return {
    localId,
    accountId: 'a',
    mode: 'new',
    toJson: [],
    ccJson: [],
    bccJson: [],
    subject: localId,
    bodyHtml: '',
    attachmentsJson: [],
    revision: 1,
    state: 'editing',
    ...over,
  } as Draft;
}

function page(drafts: Draft[], nextCursor?: string): DraftPage {
  return { drafts, nextCursor };
}

function ids(rows: Draft[]): string[] {
  return rows.map((d) => d.localId);
}

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

/** Advance past the coalescing window, then drain the resulting promises. */
async function advance(ms = 200) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
  await tick();
}

beforeEach(() => {
  draftsList.mockReset();
  listeners.clear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useDrafts query identity', () => {
  it('queries the sorted, de-duplicated scope once with the default page size', async () => {
    draftsList.mockResolvedValue(page([makeDraft('d1'), makeDraft('d2')], 'c1'));

    const { result } = renderHook(() => useDrafts(['b', 'a', 'b']));

    await waitFor(() => expect(result.current.initialLoading).toBe(false));
    expect(draftsList).toHaveBeenCalledTimes(1);
    expect(draftsList.mock.calls[0][0]).toEqual({ accountIds: ['a', 'b'], cursor: undefined, limit: 30 });
    expect(ids(result.current.rows)).toEqual(['d1', 'd2']);
    expect(result.current.nextCursor).toBe('c1');
  });

  it('clamps the page size option into the range the command accepts', async () => {
    draftsList.mockResolvedValue(page([]));

    const big = renderHook(() => useDrafts(['a'], { limit: 500 }));
    await waitFor(() => expect(big.result.current.initialLoading).toBe(false));
    expect(draftsList.mock.calls[0][0].limit).toBe(100);

    const small = renderHook(() => useDrafts(['a'], { limit: 0 }));
    await waitFor(() => expect(small.result.current.initialLoading).toBe(false));
    expect(draftsList.mock.calls[1][0].limit).toBe(1);
  });

  it('does not query an empty scope and settles as an empty list', async () => {
    const { result } = renderHook(() => useDrafts([]));
    await tick();

    expect(draftsList).not.toHaveBeenCalled();
    expect(result.current.rows).toEqual([]);
    expect(result.current.initialLoading).toBe(false);
    expect(result.current.loadingMore).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it('clears rows on a scope change and drops the superseded response', async () => {
    const first = deferred<DraftPage>();
    const second = deferred<DraftPage>();
    draftsList.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);

    const { result, rerender } = renderHook(({ scope }: { scope: string[] }) => useDrafts(scope), {
      initialProps: { scope: ['a'] },
    });
    expect(result.current.initialLoading).toBe(true);

    rerender({ scope: ['b'] });

    expect(result.current.rows).toEqual([]);
    expect(result.current.initialLoading).toBe(true);
    expect(draftsList.mock.calls[1][0].accountIds).toEqual(['b']);

    await act(async () => {
      second.resolve(page([makeDraft('b1')]));
    });
    await waitFor(() => expect(ids(result.current.rows)).toEqual(['b1']));

    await act(async () => {
      first.resolve(page([makeDraft('a1')]));
    });
    expect(ids(result.current.rows)).toEqual(['b1']);
    expect(draftsList).toHaveBeenCalledTimes(2);
  });

  it('reload replaces the head page', async () => {
    draftsList
      .mockResolvedValueOnce(page([makeDraft('d1')], 'c1'))
      .mockResolvedValueOnce(page([makeDraft('d2')]));

    const { result } = renderHook(() => useDrafts(['a']));
    await waitFor(() => expect(ids(result.current.rows)).toEqual(['d1']));

    act(() => result.current.reload());
    await waitFor(() => expect(ids(result.current.rows)).toEqual(['d2']));
    expect(draftsList.mock.calls[1][0]).toEqual({ accountIds: ['a'], cursor: undefined, limit: 30 });
    expect(result.current.nextCursor).toBeUndefined();
  });
});

describe('useDrafts paging', () => {
  it('appends the next page from the returned cursor, once per draft', async () => {
    draftsList
      .mockResolvedValueOnce(page([makeDraft('d1'), makeDraft('d2')], 'c1'))
      .mockResolvedValueOnce(page([makeDraft('d2'), makeDraft('d3')]));

    const { result } = renderHook(() => useDrafts(['a']));
    await waitFor(() => expect(result.current.rows).toHaveLength(2));

    act(() => result.current.loadMore());
    await waitFor(() => expect(result.current.rows).toHaveLength(3));

    expect(draftsList.mock.calls[1][0]).toEqual({ accountIds: ['a'], cursor: 'c1', limit: 30 });
    expect(ids(result.current.rows)).toEqual(['d1', 'd2', 'd3']);
    expect(result.current.nextCursor).toBeUndefined();
    expect(result.current.loadingMore).toBe(false);
  });

  it('keeps one page request in flight at a time', async () => {
    const second = deferred<DraftPage>();
    draftsList.mockResolvedValueOnce(page([makeDraft('d1')], 'c1')).mockReturnValueOnce(second.promise);

    const { result } = renderHook(() => useDrafts(['a']));
    await waitFor(() => expect(result.current.rows).toHaveLength(1));

    act(() => {
      result.current.loadMore();
      result.current.loadMore();
    });

    expect(draftsList).toHaveBeenCalledTimes(2);
    expect(result.current.loadingMore).toBe(true);

    await act(async () => {
      second.resolve(page([makeDraft('d2')]));
    });
    await waitFor(() => expect(ids(result.current.rows)).toEqual(['d1', 'd2']));
    expect(result.current.loadingMore).toBe(false);
  });
});

describe('useDrafts failures', () => {
  it('reports the thrown message instead of an empty list', async () => {
    draftsList.mockRejectedValue(new Error('Network unreachable'));

    const { result } = renderHook(() => useDrafts(['a']));

    await waitFor(() => expect(result.current.error).toBe('Network unreachable'));
    expect(result.current.initialLoading).toBe(false);
    expect(result.current.loadingMore).toBe(false);
    expect(result.current.rows).toEqual([]);
  });

  it('falls back to the house message for a rejection without one', async () => {
    draftsList.mockRejectedValue({ code: 'boom' });

    const { result } = renderHook(() => useDrafts(['a']));

    await waitFor(() => expect(result.current.error).toBe('Could not load drafts.'));
    expect(result.current.initialLoading).toBe(false);
  });

  it('keeps the loaded rows and clears loadingMore when a page fails', async () => {
    draftsList
      .mockResolvedValueOnce(page([makeDraft('d1'), makeDraft('d2')], 'c1'))
      .mockRejectedValueOnce(new Error('Page failed'));

    const { result } = renderHook(() => useDrafts(['a']));
    await waitFor(() => expect(result.current.rows).toHaveLength(2));

    act(() => result.current.loadMore());

    await waitFor(() => expect(result.current.error).toBe('Page failed'));
    expect(ids(result.current.rows)).toEqual(['d1', 'd2']);
    expect(result.current.loadingMore).toBe(false);
    expect(result.current.nextCursor).toBe('c1');
  });
});

describe('useDrafts store events', () => {
  it('coalesces a burst of saves into one head refresh that surfaces the new draft', async () => {
    vi.useFakeTimers();
    draftsList.mockResolvedValue(page([makeDraft('d1')], 'c1'));

    const { result } = renderHook(() => useDrafts(['a']));
    await tick();
    expect(ids(result.current.rows)).toEqual(['d1']);

    draftsList.mockResolvedValue(page([makeDraft('d9'), makeDraft('d1')], 'c1'));
    act(() => {
      emit('store:drafts', { account_id: 'a', draft_id: 'd9' });
      emit('store:drafts', { account_id: 'a', draft_id: 'd9' });
    });
    await advance();

    expect(draftsList).toHaveBeenCalledTimes(2);
    expect(draftsList.mock.calls[1][0]).toEqual({ accountIds: ['a'], cursor: undefined, limit: 30 });
    expect(ids(result.current.rows)).toEqual(['d9', 'd1']);
  });

  it('refreshes as much of the window as is loaded so paging survives', async () => {
    vi.useFakeTimers();
    draftsList
      .mockResolvedValueOnce(page([makeDraft('d1'), makeDraft('d2')], 'c1'))
      .mockResolvedValueOnce(page([makeDraft('d3')]))
      .mockResolvedValue(page([makeDraft('d9')], 'c1'));

    const { result } = renderHook(() => useDrafts(['a'], { limit: 2 }));
    await tick();
    act(() => result.current.loadMore());
    await tick();
    expect(result.current.rows).toHaveLength(3);

    act(() => emit('store:drafts', { account_id: 'a', draft_id: 'd9' }));
    await advance();

    expect(draftsList.mock.calls.at(-1)?.[0]).toEqual({
      accountIds: ['a'],
      cursor: undefined,
      limit: 4,
    });
    expect(ids(result.current.rows)).toEqual(['d9']);
  });

  it('ignores saves from accounts outside the scope', async () => {
    vi.useFakeTimers();
    draftsList.mockResolvedValue(page([makeDraft('d1')]));

    renderHook(() => useDrafts(['a']));
    await tick();

    act(() => emit('store:drafts', { account_id: 'z', draft_id: 'dz' }));
    await advance();

    expect(draftsList).toHaveBeenCalledTimes(1);
  });

  it('drops a pending refresh when the view unmounts', async () => {
    vi.useFakeTimers();
    draftsList.mockResolvedValue(page([makeDraft('d1')]));

    const { unmount } = renderHook(() => useDrafts(['a']));
    await tick();

    act(() => emit('store:drafts', { account_id: 'a', draft_id: 'd1' }));
    unmount();
    await advance(600);

    expect(draftsList).toHaveBeenCalledTimes(1);
  });
});
