import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MessageBody, MessageMeta, ThreadDetail } from '../../app/ipc/types';
import { defaultSettings } from '../../app/ipc/types';
import { useSettings } from '../../stores/settingsStore';
import { useSelection } from '../../stores/selectionStore';
import { useView } from '../../stores/viewStore';
import { resolveCommandTargets } from '../thread-list/listCommands';
import { cacheReset } from './bodyCache';
import { ThreadView } from './ThreadView';

const threadGet = vi.fn<(accountId: string, threadId: string) => Promise<ThreadDetail>>();
const messageBody = vi.fn<(accountId: string, messageId: string) => Promise<MessageBody>>();
const labelsList = vi.fn(async () => []);
const dispatchAction = vi.fn(async () => ({ undo_group: 'u1' }));

vi.mock('../../app/ipc/commands', () => ({
  api: {
    thread_get: (accountId: string, threadId: string) => threadGet(accountId, threadId),
    message_body: (accountId: string, messageId: string) => messageBody(accountId, messageId),
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

function readyBody(id: string, text: string): MessageBody {
  return {
    messageId: id,
    state: 'ready',
    text,
    remoteImageCount: 0,
    trackerCount: 0,
    darkSafe: true,
    remoteImagesAllowed: false,
  };
}

function message(id: string, isUnread: boolean, internalDate = 0): MessageMeta {
  return {
    id,
    internalDate,
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

function messageHeaders(): Element[] {
  return [...document.querySelectorAll('[data-testid^="msg-"]')];
}

beforeEach(() => {
  threadGet.mockReset();
  messageBody.mockReset();
  messageBody.mockImplementation(async (_accountId, messageId) => readyBody(messageId, `body ${messageId}`));
  labelsList.mockClear();
  dispatchAction.mockClear();
  listeners.clear();
  cacheReset();
  useSettings.setState({ settings: { ...defaultSettings, markAsRead: 'after-2s' } });
  useSelection.setState({ focusedKey: null, selectedIds: new Set(), anchorKey: null });
  useView.setState({ openThread: null, accountScope: 'all', view: { kind: 'inbox' } });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('P5-T07 expansion', () => {
  it('opens the newest page with only the newest unread messages expanded', async () => {
    const messages = Array.from({ length: 200 }, (_, i) => message(`m${i + 1}`, i >= 150, i + 1));
    threadGet.mockResolvedValue({
      accountId: 'a',
      id: 't1',
      subject: 'Long',
      labelIds: ['INBOX'],
      messages,
    });

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});

    // One page of metadata is rendered, not the whole conversation (P9.2).
    expect(messageHeaders()).toHaveLength(50);
    expect(screen.getByRole('button', { name: /Show earlier \(150\)/ })).toBeInTheDocument();

    // Fifty unread messages must not open fifty bodies: the burst stops at the
    // newest ten, and it starts from the newest.
    const expanded = messageHeaders().filter((el) => el.getAttribute('aria-expanded') === 'true');
    expect(expanded).toHaveLength(10);
    expect(expanded[expanded.length - 1]?.getAttribute('data-testid')).toBe('msg-m200');
    expect(messageBody.mock.calls.map(([, id]) => id)).toEqual([
      'm200',
      'm199',
      'm198',
      'm197',
      'm196',
      'm195',
      'm194',
      'm193',
      'm192',
      'm191',
    ]);
  });

  it('reveals older messages a page at a time', async () => {
    const messages = Array.from({ length: 60 }, (_, i) => message(`m${i + 1}`, false, i + 1));
    threadGet.mockResolvedValue({
      accountId: 'a',
      id: 't1',
      subject: 'Long',
      labelIds: ['INBOX'],
      messages,
    });

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});

    expect(messageHeaders()).toHaveLength(50);
    expect(screen.getByRole('button', { name: /Show earlier \(10\)/ })).toBeInTheDocument();

    await act(async () => {
      screen.getByRole('button', { name: /Show earlier/ }).click();
    });
    expect(messageHeaders()).toHaveLength(60);
    expect(screen.queryByRole('button', { name: /Show earlier/ })).toBeNull();
  }, 20_000);

  it('expands unread messages and the newest one in a short conversation', async () => {
    threadGet.mockResolvedValue({
      accountId: 'a',
      id: 't1',
      subject: 'Short',
      labelIds: ['INBOX'],
      messages: [1, 2, 3, 4, 5].map((n) => message(`m${n}`, n === 3, n)),
    });

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});

    const expanded = messageHeaders()
      .filter((el) => el.getAttribute('aria-expanded') === 'true')
      .map((el) => el.getAttribute('data-testid'));
    expect(expanded).toEqual(['msg-m3', 'msg-m5']);
  });
});

describe('P9.2 reader recovery and isolation', () => {
  it('shows a retry surface instead of the provider text, and recovers on retry', async () => {
    vi.useFakeTimers();
    threadGet.mockResolvedValue(detail('a', 't1', 'Subject'));
    // A transient failure: the backend keeps reporting `loading` with its own
    // wording, which must never be rendered as the message.
    messageBody.mockResolvedValue({
      messageId: 'a:t1:m1',
      state: 'loading',
      text: 'Could not reach imap.example.test',
      remoteImageCount: 0,
      trackerCount: 0,
      darkSafe: true,
      remoteImagesAllowed: false,
    });

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    // Step the backoff out to its budget: the retry surface only appears once
    // polling has given up, which is what stops an endless spinner.
    for (let i = 0; i < 12; i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2_000);
      });
    }

    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('Couldn’t load this message.');
    // The provider's wording may be shown *as* the labelled failure, never as
    // the message body.
    expect(document.querySelector('pre')?.textContent ?? '').not.toContain('Could not reach');

    messageBody.mockImplementation(async (_accountId, messageId) => readyBody(messageId, 'the real body'));
    await act(async () => {
      screen.getByRole('button', { name: 'Retry' }).click();
      await vi.advanceTimersByTimeAsync(0);
    });
    vi.useRealTimers();
    await act(async () => {});
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByText('the real body')).toBeInTheDocument();
  });

  it('renders a cached ready body without another IPC round trip', async () => {
    threadGet.mockResolvedValue(detail('a', 't1', 'Subject'));
    renderView();
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});
    expect(screen.getByText('body a:t1:m1')).toBeInTheDocument();
    const fetches = messageBody.mock.calls.length;

    // Re-opening the same message uses the bounded cache instead of re-asking
    // the backend (and re-shipping any embedded CID bytes over IPC).
    open({ accountId: 'b', threadId: 'other' });
    await act(async () => {});
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});
    expect(screen.getByText('body a:t1:m1')).toBeInTheDocument();
    expect(messageBody.mock.calls.filter(([, id]) => id === 'a:t1:m1')).toHaveLength(fetches);
  });

  it('stops polling an abandoned thread before its next IPC call', async () => {
    const gate = deferred<MessageBody>();
    threadGet.mockImplementation(async (accountId, threadId) =>
      detail(accountId, threadId, `Subject ${threadId}`),
    );
    let firstCallAborted = false;
    messageBody.mockImplementation(async (_accountId, messageId) => {
      if (messageId.startsWith('a:t1')) {
        const body = await gate.promise;
        firstCallAborted = true;
        return body;
      }
      return readyBody(messageId, `body ${messageId}`);
    });

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});
    const outstanding = messageBody.mock.calls.length;

    // Navigate away while the fetch is in flight.
    open({ accountId: 'b', threadId: 't2' });
    await act(async () => {});
    gate.resolve(readyBody('a:t1:m1', 'late body'));
    await act(async () => {});

    expect(firstCallAborted).toBe(true);
    // The abandoned message was never retried, and its late body did not leak
    // into the thread that is actually open.
    expect(messageBody.mock.calls.filter(([, id]) => id.startsWith('a:t1')).length).toBe(outstanding);
    expect(screen.queryByText('late body')).toBeNull();
    expect(screen.getByText('body b:t2:m1')).toBeInTheDocument();
  });

  it('keeps the same message id in two accounts apart', async () => {
    threadGet.mockImplementation(async (accountId, threadId) => ({
      accountId,
      id: threadId,
      subject: `Subject ${accountId}`,
      labelIds: ['INBOX'],
      messages: [message('dup-m1', true, 1)],
    }));
    messageBody.mockImplementation(async (accountId, messageId) =>
      readyBody(messageId, `body for ${accountId}`),
    );

    renderView();
    open({ accountId: 'a', threadId: 't1' });
    await act(async () => {});
    expect(screen.getByText('body for a')).toBeInTheDocument();

    open({ accountId: 'b', threadId: 't1' });
    await act(async () => {});
    expect(screen.getByText('body for b')).toBeInTheDocument();
    expect(screen.queryByText('body for a')).toBeNull();
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
