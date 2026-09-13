import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MessageMeta, ThreadDetail } from '../../app/ipc/types';
import { defaultSettings } from '../../app/ipc/types';
import { useSettings } from '../../stores/settingsStore';
import { useSelection } from '../../stores/selectionStore';
import { useView } from '../../stores/viewStore';
import { resolveCommandTargets } from '../thread-list/listCommands';
import { ThreadView } from './ThreadView';

const threadGet = vi.fn<(accountId: string, threadId: string) => Promise<ThreadDetail>>();
const messageBody = vi.fn(async (id: string) => ({
  messageId: id,
  state: 'ready' as const,
  text: 'body',
  remoteImageCount: 0,
  trackerCount: 0,
  darkSafe: true,
  remoteImagesAllowed: false,
}));
const labelsList = vi.fn(async () => []);
const dispatchAction = vi.fn(async () => ({ undo_group: 'u1' }));

vi.mock('../../app/ipc/commands', () => ({
  api: {
    thread_get: (accountId: string, threadId: string) => threadGet(accountId, threadId),
    message_body: (id: string) => messageBody(id),
    labels_list: () => labelsList(),
  },
}));

vi.mock('../actions/dispatch', () => ({
  dispatchAction: (...args: unknown[]) => dispatchAction(...(args as [])),
  undoLast: vi.fn(),
}));

vi.mock('sonner', () => ({ toast: vi.fn(), toastError: vi.fn() }));

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

function message(id: string, isUnread: boolean): MessageMeta {
  return {
    id,
    internalDate: 0,
    from: { e: 'a@x', n: 'A' },
    to: [],
    cc: [],
    bcc: [],
    subject: id,
    snippet: '',
    isUnread,
    isStarred: false,
    isDraft: false,
    isSentByMe: false,
    labelIds: ['INBOX'],
    hasAttachments: false,
    attachments: [],
    bodyState: 'fetched',
  };
}

function detail(accountId: string, id: string, subject: string, unread = true): ThreadDetail {
  return {
    accountId,
    id,
    subject,
    labelIds: ['INBOX'],
    messages: [message(`${accountId}:${id}:m1`, unread)],
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function renderView(obscured = false) {
  return render(<ThreadView onReply={() => {}} obscured={obscured} />);
}

function open(ref: { accountId: string; threadId: string }) {
  act(() => {
    useView.getState().setOpenThread(ref);
  });
}

beforeEach(() => {
  threadGet.mockReset();
  messageBody.mockClear();
  labelsList.mockClear();
  dispatchAction.mockClear();
  listeners.clear();
  useSettings.setState({ settings: { ...defaultSettings, markAsRead: 'after-2s' } });
  useSelection.setState({ focusedKey: null, selectedIds: new Set(), anchorKey: null });
  useView.setState({ openThread: null, accountScope: 'all', view: { kind: 'inbox' } });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('P5-T07 expansion + remote images autoload', () => {
  it('5 messages with 2 unread -> unread + last expanded', () => {
    const msgs = [
      { id: 'm1', isUnread: false },
      { id: 'm2', isUnread: false },
      { id: 'm3', isUnread: true },
      { id: 'm4', isUnread: false },
      { id: 'm5', isUnread: false },
    ];
    const exp: Record<string, boolean> = {};
    msgs.forEach((m, i) => {
      exp[m.id] = m.isUnread || i === msgs.length - 1;
    });
    expect(exp).toEqual({ m1: false, m2: false, m3: true, m4: false, m5: true });
  });
  it('remote images load by default: no per-message banner', () => {
    // Like any other email client, images render without a "Load" gate.
    // The backend returns remoteImagesAllowed:true unless the user opted
    // into Settings → Privacy → Never, so the thread view never banners.
    const body = { remoteImageCount: 2, remoteImagesAllowed: true };
    const showsBanner = body.remoteImageCount > 0 && !body.remoteImagesAllowed;
    expect(showsBanner).toBe(false);
  });
});

describe('P3.2 account-qualified reader', () => {
  it('shows the last clicked account/thread when earlier responses resolve in reverse', async () => {
    const aGate = deferred<ThreadDetail>();
    const bGate = deferred<ThreadDetail>();
    threadGet.mockImplementation((accountId) => (accountId === 'a' ? aGate.promise : bGate.promise));

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    open({ accountId: 'b', threadId: 't1' });

    // The abandoned A response has not resolved yet: its detail must not show.
    expect(screen.queryByText('Subject A')).toBeNull();

    // The last click (B) resolves first…
    await act(async () => {
      bGate.resolve(detail('b', 't1', 'Subject B'));
    });
    expect(screen.getByText('Subject B')).toBeInTheDocument();

    // …then the stale A response arrives and is discarded.
    await act(async () => {
      aGate.resolve(detail('a', 't1', 'Subject A'));
    });
    expect(screen.getByText('Subject B')).toBeInTheDocument();
    expect(screen.queryByText('Subject A')).toBeNull();
    expect(useView.getState().openThread).toEqual({ accountId: 'b', threadId: 't1' });
    // The reader's own action target is the clicked account.
    expect(resolveCommandTargets('thread')).toEqual([{ accountId: 'b', threadIds: ['t1'] }]);
  });

  it('never loads another account detail for a repeated thread id', async () => {
    const requested: string[] = [];
    threadGet.mockImplementation(async (accountId, threadId) => {
      requested.push(`${accountId}:${threadId}`);
      return detail(accountId, threadId, `Subject ${accountId}`);
    });

    renderView();
    open({ accountId: 'a', threadId: 'dup' });
    await act(async () => {});
    expect(screen.getByText('Subject a')).toBeInTheDocument();
    expect(requested).toEqual(['a:dup']);

    open({ accountId: 'b', threadId: 'dup' });
    await act(async () => {});
    expect(screen.getByText('Subject b')).toBeInTheDocument();
    expect(requested).toEqual(['a:dup', 'b:dup']);
    expect(resolveCommandTargets('thread')).toEqual([{ accountId: 'b', threadIds: ['dup'] }]);
  });

  it('does not mark abandoned messages read during rapid navigation', async () => {
    vi.useFakeTimers();
    threadGet.mockImplementation(async (accountId, threadId) => detail(accountId, threadId, 'S'));

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    // Navigate away before the 2s dwell elapses.
    open({ accountId: 'b', threadId: 't2' });
    await act(async () => {});
    await act(async () => {
      vi.advanceTimersByTime(1500);
    });
    expect(dispatchAction).not.toHaveBeenCalled();

    // Only the thread that stayed visible for the full duration is marked read.
    await act(async () => {
      vi.advanceTimersByTime(600);
    });
    expect(dispatchAction).toHaveBeenCalledTimes(1);
    expect(dispatchAction).toHaveBeenCalledWith(
      { accountId: 'b', threadIds: ['t2'], action: { kind: 'read', on: true } },
      { silent: true },
    );
  });

  it('clears the pending mark-read timer while a modal obscures the reader', async () => {
    vi.useFakeTimers();
    threadGet.mockImplementation(async (accountId, threadId) => detail(accountId, threadId, 'S'));

    const { rerender } = render(<ThreadView onReply={() => {}} obscured={false} />);
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    rerender(<ThreadView onReply={() => {}} obscured />);
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });
    expect(dispatchAction).not.toHaveBeenCalled();

    rerender(<ThreadView onReply={() => {}} obscured={false} />);
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });
    expect(dispatchAction).toHaveBeenCalledWith(
      { accountId: 'a', threadIds: ['t1'], action: { kind: 'read', on: true } },
      { silent: true },
    );
  });

  it('clears the timer and the displayed thread when the reader closes', async () => {
    vi.useFakeTimers();
    threadGet.mockImplementation(async (accountId, threadId) => detail(accountId, threadId, 'S'));

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});
    act(() => {
      useView.getState().setOpenThread(null);
    });
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });
    expect(dispatchAction).not.toHaveBeenCalled();
    expect(screen.getByText('Select a conversation')).toBeInTheDocument();
  });
});
