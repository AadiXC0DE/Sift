import { api } from '../../app/ipc/commands';
import type { MessageBody } from '../../app/ipc/types';
import { cacheSet } from './bodyCache';

/**
 * Foreground body fetching for the reader (P9.2).
 *
 * The previous poller was a bare `for` loop with no cancellation: navigating
 * away left it running, its next IPC still went out, and after sixteen
 * attempts it fabricated a `ready` body out of the provider's error text.
 * Everything here is therefore (a) abortable *before* the next IPC, (b)
 * bounded, and (c) forbidden from inventing a terminal state.
 */

/** Total attempts before the reader shows an explicit retry surface. */
export const POLL_ATTEMPTS = 10;
/** The backend single-flights an in-flight fetch, so two is a polite burst. */
export const FETCH_CONCURRENCY = 2;

export type BodyOutcome =
  /** A terminal `ready` body was delivered. */
  | 'ready'
  /** The backend reported a terminal failure; retry is a user decision. */
  | 'error'
  /** Retryable (offline, transient provider error) and the budget ran out. */
  | 'loading'
  /** Navigation/supersession aborted the poll. Nothing was delivered. */
  | 'aborted';

/**
 * Sleep that wakes early when the reader navigates. Written with the executor
 * form because `Promise.withResolvers` is ES2024 and this project's `lib` is
 * ES2022; the abort listener is removed on either path so a long-lived signal
 * cannot accumulate listeners.
 */
function abortableSleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', done);
      resolve();
    };
    const timer = setTimeout(done, ms);
    signal.addEventListener('abort', done, { once: true });
  });
}

/**
 * Poll one message body until it reaches a terminal state or the attempt
 * budget is spent. Never promotes a retryable failure to ready/error and never
 * writes anything after the signal aborts.
 */
export async function pollBody(
  accountId: string,
  messageId: string,
  onUpdate: (body: MessageBody) => void,
  signal: AbortSignal,
): Promise<BodyOutcome> {
  let delay = 250;
  for (let attempt = 0; attempt < POLL_ATTEMPTS; attempt++) {
    // Checked here as well as after the await: the point of the check is that
    // a navigated-away reader issues no further IPC at all.
    if (signal.aborted) return 'aborted';
    if (attempt > 0) {
      await abortableSleep(delay, signal);
      if (signal.aborted) return 'aborted';
      delay = Math.min(Math.round(delay * 1.35), 2000);
    }
    let body: MessageBody;
    try {
      body = await api.message_body(accountId, messageId);
    } catch {
      // A transport failure is retryable; it is not evidence about the message.
      continue;
    }
    if (signal.aborted) return 'aborted';
    if (body.state === 'ready') {
      cacheSet(accountId, messageId, body);
      onUpdate(body);
      return 'ready';
    }
    onUpdate(body);
    if (body.state === 'error') return 'error';
    // `loading` is a retryable state: keep going, cache nothing.
  }
  // Budget spent without a terminal body: the reader shows an explicit retry
  // surface. A fabricated `ready` here is exactly the P9.2 defect.
  return 'loading';
}

/**
 * Fetch the given messages newest first, at most `concurrency` in flight, so a
 * 200-message conversation paints its newest bodies immediately instead of
 * walking from the oldest one.
 */
export async function fetchBodiesNewestFirst(
  accountId: string,
  messageIdsNewestFirst: string[],
  onUpdate: (messageId: string, body: MessageBody) => void,
  onSettled: (messageId: string, outcome: BodyOutcome) => void,
  signal: AbortSignal,
  concurrency = FETCH_CONCURRENCY,
): Promise<void> {
  let next = 0;
  const workerCount = Math.max(1, Math.min(concurrency, messageIdsNewestFirst.length));
  const worker = async () => {
    for (;;) {
      if (signal.aborted) return;
      const index = next++;
      const messageId = messageIdsNewestFirst[index];
      if (messageId === undefined) return;
      const outcome = await pollBody(
        accountId,
        messageId,
        (body) => {
          if (!signal.aborted) onUpdate(messageId, body);
        },
        signal,
      );
      if (signal.aborted) return;
      onSettled(messageId, outcome);
    }
  };
  await Promise.all(Array.from({ length: workerCount }, worker));
}

/** Message ids in newest-first order; ties keep the thread's own ordering. */
export function idsNewestFirst(messages: { id: string; internalDate: number }[]): string[] {
  return messages
    .map((m, index) => ({ id: m.id, internalDate: m.internalDate, index }))
    .sort((a, b) => b.internalDate - a.internalDate || b.index - a.index)
    .map((m) => m.id);
}
