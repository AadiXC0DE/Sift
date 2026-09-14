import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MessageBody } from '../../app/ipc/types';
import { cacheGet, cacheReset } from './bodyCache';
import { fetchBodiesNewestFirst, idsNewestFirst, pollBody } from './bodyFetch';

const messageBody = vi.fn<(accountId: string, messageId: string) => Promise<MessageBody>>();

vi.mock('../../app/ipc/commands', () => ({
  api: {
    message_body: (accountId: string, messageId: string) => messageBody(accountId, messageId),
  },
}));

function ready(id: string, text = 'body'): MessageBody {
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

function loading(id: string, text?: string): MessageBody {
  return {
    messageId: id,
    state: 'loading',
    text,
    remoteImageCount: 0,
    trackerCount: 0,
    darkSafe: true,
    remoteImagesAllowed: false,
  };
}

beforeEach(() => {
  messageBody.mockReset();
  cacheReset();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('P9.2 polling', () => {
  it('stops before the next IPC call once the reader navigates', async () => {
    messageBody.mockResolvedValue(loading('m1', 'still working'));
    const controller = new AbortController();
    const updates: MessageBody[] = [];
    const outcome = pollBody('a', 'm1', (b) => updates.push(b), controller.signal);

    await vi.advanceTimersByTimeAsync(300);
    expect(messageBody.mock.calls.length).toBeGreaterThanOrEqual(1);
    const callsBefore = messageBody.mock.calls.length;
    controller.abort();
    await vi.advanceTimersByTimeAsync(10_000);

    expect(await outcome).toBe('aborted');
    expect(messageBody.mock.calls.length).toBe(callsBefore);
  });

  it('issues no IPC at all when the signal is already aborted', async () => {
    const controller = new AbortController();
    controller.abort();
    expect(await pollBody('a', 'm1', () => {}, controller.signal)).toBe('aborted');
    expect(messageBody).not.toHaveBeenCalled();
  });

  it('never promotes a retryable failure into ready or error content', async () => {
    messageBody.mockResolvedValue(loading('m1', 'Could not reach the server'));
    const updates: MessageBody[] = [];
    const outcome = pollBody('a', 'm1', (b) => updates.push(b), new AbortController().signal);
    await vi.advanceTimersByTimeAsync(120_000);

    expect(await outcome).toBe('loading');
    expect(updates.every((b) => b.state === 'loading')).toBe(true);
    // Nor is the provider's wording promoted into cached content.
    expect(cacheGet('a', 'm1')).toBeUndefined();
  });

  it('retries a transport failure and delivers the body once it arrives', async () => {
    messageBody.mockRejectedValueOnce(new Error('ipc down')).mockResolvedValueOnce(ready('m1', 'arrived'));
    const updates: MessageBody[] = [];
    const outcome = pollBody('a', 'm1', (b) => updates.push(b), new AbortController().signal);
    await vi.advanceTimersByTimeAsync(5000);

    expect(await outcome).toBe('ready');
    expect(updates.map((b) => b.text)).toEqual(['arrived']);
    expect(cacheGet('a', 'm1')?.text).toBe('arrived');
  });

  it('treats a terminal backend error as terminal and caches nothing', async () => {
    messageBody.mockResolvedValue({ ...ready('m1'), state: 'error', text: 'message not found' });
    const outcome = pollBody('a', 'm1', () => {}, new AbortController().signal);
    expect(await outcome).toBe('error');
    expect(messageBody).toHaveBeenCalledTimes(1);
    expect(cacheGet('a', 'm1')).toBeUndefined();
  });
});

describe('P9.2 fetch order', () => {
  it('orders expanded messages newest first', () => {
    const ids = idsNewestFirst([
      { id: 'old', internalDate: 1 },
      { id: 'new', internalDate: 3 },
      { id: 'mid', internalDate: 2 },
    ]);
    expect(ids).toEqual(['new', 'mid', 'old']);
  });

  it('starts the newest messages first and keeps two fetches in flight', async () => {
    let inFlight = 0;
    let peak = 0;
    const started: string[] = [];
    const gate = new Map<string, (b: MessageBody) => void>();
    messageBody.mockImplementation(async (_accountId, messageId) => {
      started.push(messageId);
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      const body = await new Promise<MessageBody>((resolve) => gate.set(messageId, resolve));
      inFlight -= 1;
      return body;
    });

    const settled: string[] = [];
    const done = fetchBodiesNewestFirst(
      'a',
      ['m5', 'm4', 'm3', 'm2', 'm1'],
      () => {},
      (id) => settled.push(id),
      new AbortController().signal,
    );

    // Let the two workers reach their first await.
    await vi.advanceTimersByTimeAsync(0);
    expect(started).toEqual(['m5', 'm4']);
    expect(peak).toBe(2);

    // Releasing the newest lets the pool walk down the list, still two at a time.
    gate.get('m5')!(ready('m5'));
    gate.get('m4')!(ready('m4'));
    await vi.advanceTimersByTimeAsync(0);
    expect(started).toEqual(['m5', 'm4', 'm3', 'm2']);
    expect(peak).toBe(2);

    gate.get('m3')!(ready('m3'));
    gate.get('m2')!(ready('m2'));
    await vi.advanceTimersByTimeAsync(0);
    gate.get('m1')!(ready('m1'));
    await done;

    expect(started).toEqual(['m5', 'm4', 'm3', 'm2', 'm1']);
    expect(settled).toEqual(['m5', 'm4', 'm3', 'm2', 'm1']);
    expect(peak).toBe(2);
  });

  it('stops the pool when the thread is left mid-flight', async () => {
    const started: string[] = [];
    messageBody.mockImplementation(async (_accountId, messageId) => {
      started.push(messageId);
      return ready(messageId);
    });
    const controller = new AbortController();
    controller.abort();
    await fetchBodiesNewestFirst(
      'a',
      ['m1', 'm2', 'm3'],
      () => {},
      () => {},
      controller.signal,
    );
    expect(started).toEqual([]);
  });
});
