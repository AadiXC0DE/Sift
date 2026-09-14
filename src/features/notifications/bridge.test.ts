import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useNotificationBridge } from './bridge';

const bridge = vi.hoisted(() => {
  const state = {
    registered: new Map<string, (payload: unknown) => void>(),
    unlistened: [] as string[],
    deferredResolvers: new Map<string, (unlisten: () => void) => void>(),
    defer: false,
  };
  const on = (event: string, handler: (payload: unknown) => void) => {
    state.registered.set(event, handler);
    const unlisten = () => {
      state.unlistened.push(event);
    };
    if (!state.defer) return Promise.resolve(unlisten);
    return new Promise<() => void>((resolve) => {
      state.deferredResolvers.set(event, resolve);
    });
  };
  return { state, on };
});

vi.mock('../../app/ipc/events', () => ({ on: bridge.on }));

const fire = (event: string, payload: unknown) => {
  const handler = bridge.state.registered.get(event);
  expect(handler, `${event} was never subscribed`).toBeDefined();
  act(() => {
    handler?.(payload);
  });
};

beforeEach(() => {
  bridge.state.registered.clear();
  bridge.state.unlistened.length = 0;
  bridge.state.deferredResolvers.clear();
  bridge.state.defer = false;
  vi.unstubAllGlobals();
});

const noop = () => {};

describe('useNotificationBridge (P8.4)', () => {
  it('routes nav:open-thread to the latest handler without a stale closure', () => {
    const first = vi.fn();
    const second = vi.fn();
    const onBadge = vi.fn();
    const { rerender } = renderHook(
      ({ onOpenThread }: { onOpenThread: (r: { accountId: string; threadId: string }) => void }) =>
        useNotificationBridge({ onOpenThread, onBadge }),
      { initialProps: { onOpenThread: first } },
    );

    fire('nav:open-thread', { accountId: 'a1', threadId: 't1' });
    expect(first).toHaveBeenCalledWith({ accountId: 'a1', threadId: 't1' });

    rerender({ onOpenThread: second });
    fire('nav:open-thread', { accountId: 'a2', threadId: 't2' });

    expect(second).toHaveBeenCalledWith({ accountId: 'a2', threadId: 't2' });
    expect(first).toHaveBeenCalledTimes(1);
  });

  it('reports badge counts and ignores notify:new without touching any notification API', () => {
    const NotificationCtor = vi.fn();
    const AudioCtor = vi.fn();
    vi.stubGlobal('Notification', NotificationCtor);
    vi.stubGlobal('AudioContext', AudioCtor);

    const onOpenThread = vi.fn();
    const onBadge = vi.fn();
    renderHook(() => useNotificationBridge({ onOpenThread, onBadge }));

    fire('badge:update', { count: 7 });
    expect(onBadge).toHaveBeenCalledWith(7);

    // Rust already delivered (or suppressed) the banner; the UI must not
    // re-notify, and must never navigate on incoming mail.
    fire('notify:new', {
      accountId: 'a1',
      threadId: 't1',
      title: 'Boss',
      subject: 'Quarterly numbers',
      body: 'Quarterly numbers',
      hidden: false,
    });

    expect(onOpenThread).not.toHaveBeenCalled();
    expect(onBadge).toHaveBeenCalledTimes(1);
    expect(NotificationCtor).not.toHaveBeenCalled();
    expect(AudioCtor).not.toHaveBeenCalled();
  });

  it('subscribes to every backend event and releases them on unmount', async () => {
    const handlers = { onOpenThread: noop, onBadge: noop };
    const { unmount } = renderHook(() => useNotificationBridge(handlers));

    expect([...bridge.state.registered.keys()].sort()).toEqual([
      'badge:update',
      'nav:open-thread',
      'notify:new',
    ]);

    unmount();
    // The unlisten handles arrive on the promise `on` returns.
    await act(async () => {
      await Promise.resolve();
    });
    expect(bridge.state.unlistened.sort()).toEqual(['badge:update', 'nav:open-thread', 'notify:new']);
  });

  it('releases a subscription that resolves after unmount', async () => {
    bridge.state.defer = true;
    const handlers = { onOpenThread: noop, onBadge: noop };
    const { unmount } = renderHook(() => useNotificationBridge(handlers));
    unmount();

    await act(async () => {
      for (const [event, resolve] of bridge.state.deferredResolvers) {
        resolve(() => {
          bridge.state.unlistened.push(event);
        });
      }
      await Promise.resolve();
    });

    expect(bridge.state.unlistened.sort()).toEqual(['badge:update', 'nav:open-thread', 'notify:new']);
  });
});
