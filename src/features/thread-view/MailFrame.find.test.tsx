import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render } from '@testing-library/react';
import { MailFrame } from './MailFrame';
import { clearFindResults, intendedFindTarget, listFinders, subscribeFindRequests } from './find';

vi.mock('../../app/ipc/commands', () => ({
  api: { app_open_url: vi.fn() },
}));

function frameOf(container: HTMLElement): HTMLIFrameElement {
  return container.querySelector('iframe')!;
}

function srcdocOf(container: HTMLElement): string {
  return frameOf(container).getAttribute('srcdoc')!;
}

/** The token the shim was built with, read back out of the injected script. */
function tokenOf(container: HTMLElement): string {
  const match = srcdocOf(container).match(/data-token="([0-9a-f]+)"/);
  if (!match) throw new Error('shim token missing');
  return match[1];
}

/** Post a message as the frame would, so the parent's validation is exercised. */
function postFromFrame(
  container: HTMLElement,
  data: Record<string, unknown>,
  opts?: { token?: string | null; source?: unknown },
): void {
  const iframe = frameOf(container);
  const token = opts?.token === undefined ? tokenOf(container) : opts.token;
  act(() => {
    window.dispatchEvent(
      new MessageEvent('message', {
        data: { __sift: true, ...(token ? { token } : {}), ...data },
        source: (opts?.source ?? iframe.contentWindow) as MessageEventSource,
      }),
    );
  });
}

/** The frame rendered for `messageId`, as the find bar would reach it. */
function frameFinder(messageId: string) {
  const finder = listFinders().find((f) => f.messageId === messageId);
  if (!finder) throw new Error(`no frame registered for ${messageId}`);
  return finder;
}

let subscriptions: Array<() => void> = [];

beforeEach(() => {
  clearFindResults();
  subscriptions = [];
});

afterEach(() => {
  for (const off of subscriptions) off();
  vi.restoreAllMocks();
});

describe('P9.3 MailFrame find channel', () => {
  it('posts a search into the frame and resolves it with the frame answer', async () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const posted = vi.spyOn(frameOf(container).contentWindow!, 'postMessage');

    const pending = frameFinder('m1').find('cat', -1, true);
    expect(posted).toHaveBeenCalledWith(
      {
        __siftFindReq: true,
        token: tokenOf(container),
        type: 'find',
        q: 'cat',
        direction: -1,
        reset: true,
      },
      '*',
    );

    postFromFrame(container, { type: 'find-result', count: 2, index: 1 });
    await expect(pending).resolves.toEqual({ count: 2, index: 1 });
  });

  it('takes the count only from this frame, with this frame token', async () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);

    const pending = frameFinder('m1').find('cat', 1, true);
    // A neighbour frame, or another window, must not answer for this one.
    postFromFrame(container, { type: 'find-result', count: 5, index: 2 }, { token: 'not-the-token' });
    postFromFrame(container, { type: 'find-result', count: 5, index: 2 }, { source: window });
    postFromFrame(container, { type: 'find-result', count: 9, index: 4 });

    await expect(pending).resolves.toEqual({ count: 9, index: 4 });
  });

  it('opens the app find bar when Cmd+F is pressed inside the message', () => {
    // The first frame on screen is the one a naive implementation would pick.
    render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const second = render(<MailFrame messageId="m2" html="<p>hi</p>" allowed darkSafe />);
    const keys: KeyboardEvent[] = [];
    const listener = (e: KeyboardEvent) => keys.push(e);
    window.addEventListener('keydown', listener);

    postFromFrame(second.container, { type: 'find-open' });

    window.removeEventListener('keydown', listener);
    // Find targets the message the user was reading, and the app's own keymap
    // path runs as if the chord had been pressed over the reader.
    expect(intendedFindTarget()).toBe('m2');
    expect(keys.map((e) => e.key)).toEqual(['f']);
    expect(keys[0].metaKey).toBe(true);
  });

  it('relays Escape from inside the message onto the app window', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const seen = vi.fn();
    window.addEventListener('keydown', seen);

    postFromFrame(container, { type: 'find-escape' });

    window.removeEventListener('keydown', seen);
    expect(seen).toHaveBeenCalledTimes(1);
    expect((seen.mock.calls[0][0] as KeyboardEvent).key).toBe('Escape');
  });

  it('makes the message the user clicked into the one find searches', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);

    fireEvent.focus(frameOf(container));

    expect(intendedFindTarget()).toBe('m1');
  });

  it('drops highlights when a new body reloads the frame', () => {
    const requests: string[] = [];
    subscriptions.push(
      subscribeFindRequests((req) => requests.push(`${req.messageId}:${req.q}:${req.reset}`)),
    );

    const { container, rerender } = render(<MailFrame messageId="m1" html="<p>one</p>" allowed darkSafe />);
    const posted = vi.spyOn(frameOf(container).contentWindow!, 'postMessage');
    rerender(<MailFrame messageId="m1" html="<p>two</p>" allowed darkSafe />);

    // Whatever the previous document highlighted is not in the new one, while
    // the find bar still shows a count: the frame is told to drop it.
    expect(requests[requests.length - 1]).toBe('m1::true');
    expect(posted).toHaveBeenCalledWith(
      {
        __siftFindReq: true,
        token: tokenOf(container),
        type: 'find',
        q: '',
        direction: 1,
        reset: true,
      },
      '*',
    );
  });
});
