import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { FindBar } from './FindBar';
import {
  clearFindResults,
  createFrameFinder,
  focusFinder,
  registerFinder,
  resolveFindResult,
  subscribeFindRequests,
  type FindHit,
  type FindRequest,
} from './find';
import { clearSurfaces, handleEscape, hasBlockingSurface } from '../../ui/overlayStack';

/** Past the 120 ms debounce, with the resulting state updates flushed. */
async function settleDebounce() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(150);
  });
}

const findInput = () => screen.getByLabelText('Find in message');

let requests: FindRequest[] = [];
let registered: Array<() => void> = [];
let subscriptions: Array<() => void> = [];

/** Registers a frame for the duration of one test. */
function addFrame(messageId: string): void {
  registered.push(registerFinder(createFrameFinder(messageId)));
}

/** Records requests; the unsubscribes keep one test from answering another's. */
function onRequest(fn: (req: FindRequest) => void): void {
  subscriptions.push(subscribeFindRequests(fn));
}

/**
 * Answers every incoming request the way a live frame would: through the same
 * resolveFindResult the MailFrame message handler calls.
 */
function answerWith(reply: (req: FindRequest) => FindHit): void {
  onRequest((req) => resolveFindResult(req.messageId, reply(req)));
}

beforeEach(() => {
  vi.useFakeTimers();
  clearFindResults();
  requests = [];
  registered = [];
  subscriptions = [];
  onRequest((req) => requests.push(req));
});

afterEach(() => {
  for (const off of subscriptions) off();
  for (const unregister of registered) unregister();
  clearSurfaces();
  vi.useRealTimers();
});

describe('P9.3 find bar', () => {
  it('reports the current match of the total, and says so when there is none', async () => {
    addFrame('m1');
    answerWith((req) => (req.q === 'budget' ? { count: 12, index: 3 } : { count: 0, index: 0 }));

    render(<FindBar open onClose={() => {}} />);
    fireEvent.change(findInput(), { target: { value: 'budget' } });
    await settleDebounce();
    expect(screen.getByText('3 of 12')).toBeTruthy();

    fireEvent.change(findInput(), { target: { value: 'no such text' } });
    await settleDebounce();
    expect(screen.getByText('No matches')).toBeTruthy();
  });

  it('asks the frame to step, and steps backwards on Shift+Enter', async () => {
    addFrame('m1');
    answerWith(() => ({ count: 3, index: 1 }));
    render(<FindBar open onClose={() => {}} />);

    fireEvent.change(findInput(), { target: { value: 'budget' } });
    await settleDebounce();
    requests = [];

    await act(async () => {
      fireEvent.keyDown(findInput(), { key: 'Enter' });
      fireEvent.keyDown(findInput(), { key: 'Enter', shiftKey: true });
      fireEvent.click(screen.getByRole('button', { name: 'Next match' }));
      fireEvent.click(screen.getByRole('button', { name: 'Previous match' }));
    });

    // A step never re-scans: a reset would jump back to match 1.
    expect(requests).toEqual([
      { messageId: 'm1', q: 'budget', direction: 1, reset: false },
      { messageId: 'm1', q: 'budget', direction: -1, reset: false },
      { messageId: 'm1', q: 'budget', direction: 1, reset: false },
      { messageId: 'm1', q: 'budget', direction: -1, reset: false },
    ]);
  });

  it('says there is no message to search instead of reporting zero matches', async () => {
    render(<FindBar open onClose={() => {}} />);

    fireEvent.change(findInput(), { target: { value: 'budget' } });
    await settleDebounce();

    expect(screen.getByText('No message open')).toBeTruthy();
    expect(screen.queryByText('No matches')).toBeNull();
    expect(requests).toEqual([]);
  });

  it('searches the frame the user last read, not merely the first one', async () => {
    addFrame('m1');
    addFrame('m2');
    answerWith((req) => (req.messageId === 'm2' ? { count: 7, index: 2 } : { count: 1, index: 1 }));

    render(<FindBar open onClose={() => {}} />);
    act(() => {
      focusFinder('m2');
    });
    fireEvent.change(findInput(), { target: { value: 'budget' } });
    await settleDebounce();

    expect(screen.getByText('2 of 7')).toBeTruthy();
    expect(requests.map((r) => r.messageId)).toEqual(['m2']);
  });

  it('closes on Escape without letting the key through to the app, and clears every frame', () => {
    addFrame('m1');
    addFrame('m2');
    const onClose = vi.fn();
    // What the app's window listener would see if Escape escaped the bar.
    const appSaw = vi.fn();
    window.addEventListener('keydown', appSaw);

    const { rerender } = render(<FindBar open onClose={onClose} />);
    expect(hasBlockingSurface()).toBe(true);

    fireEvent.change(findInput(), { target: { value: 'budget' } });
    fireEvent.keyDown(findInput(), { key: 'Escape' });

    expect(onClose).toHaveBeenCalledTimes(1);
    // Backing out of the conversation must not happen at the same time.
    expect(appSaw).not.toHaveBeenCalled();

    // Closing the bar hands every frame back its untouched text.
    rerender(<FindBar open={false} onClose={onClose} />);
    expect(
      requests
        .filter((r) => r.q === '')
        .map((r) => r.messageId)
        .sort(),
    ).toEqual(['m1', 'm2']);
    expect(hasBlockingSurface()).toBe(false);

    window.removeEventListener('keydown', appSaw);
  });

  it('owns Escape through the overlay stack, so the app can close it too', () => {
    const onClose = vi.fn();
    const { unmount } = render(<FindBar open onClose={onClose} />);

    act(() => {
      expect(handleEscape()).toBe('handled');
    });
    expect(onClose).toHaveBeenCalledTimes(1);

    unmount();
    expect(hasBlockingSurface()).toBe(false);
  });

  it('close button dismisses the bar', () => {
    const onClose = vi.fn();
    render(<FindBar open onClose={onClose} />);

    fireEvent.click(screen.getByRole('button', { name: 'Close find' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
