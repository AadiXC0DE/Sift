/**
 * P8.4 — the frontend's only notification listener.
 *
 * Rust owns native delivery and the dock badge: it decides whether a message
 * notifies, groups bursts, respects the user's filter/privacy settings and
 * plays the sound through the OS. The frontend therefore never requests
 * permission here, never constructs an OS notification and never plays an
 * oscillator — it only reacts to the two navigation events the backend emits.
 *
 * The event names live here rather than in `app/ipc/events.ts` because that
 * module is the legacy `SiftEvents` list owned by the shared wiring; only the
 * payload shapes belong to this feature.
 */
import { useEffect, useRef } from 'react';
import { on } from '../../app/ipc/events';

/** Emitted by Rust *after* it delivered (or deliberately suppressed) a banner. */
interface NotifyNewPayload {
  accountId: string;
  threadId: string;
  title: string;
  subject: string | null;
  body: string;
  hidden: boolean;
}

/** Emitted when the user clicks the OS notification; the window is focused. */
interface OpenThreadPayload {
  accountId: string;
  threadId: string;
}

interface BadgePayload {
  count: number;
}

export interface NotificationBridgeHandlers {
  onOpenThread: (ref: { accountId: string; threadId: string }) => void;
  onBadge: (count: number) => void;
}

/**
 * Subscribes to `notify:new`, `nav:open-thread` and `badge:update`; returns
 * nothing. `handlers` is read through a ref so a listener registered once can
 * never navigate to a thread chosen by an older render.
 */
export function useNotificationBridge(handlers: NotificationBridgeHandlers): void {
  const latest = useRef(handlers);
  useEffect(() => {
    latest.current = handlers;
  });

  useEffect(() => {
    let disposed = false;
    const unsubs: (() => void)[] = [];

    // `on` resolves to the unlisten fn; if the component unmounts first we
    // drop the subscription as soon as it arrives instead of leaking it.
    const track = (pending: Promise<() => void>) => {
      pending
        .then((unlisten) => {
          if (disposed) unlisten();
          else unsubs.push(unlisten);
        })
        .catch(() => {
          // No event bridge (plain browser build, e2e): navigation and badge
          // events simply never arrive, which is the pre-P8.4 behaviour.
        });
    };

    // Observed for its payload contract only. `notify:new` must never reach
    // the user: no OS notification, no in-app toast, no navigation. Rust
    // emits `nav:open-thread` when the user actually clicks the banner, so
    // acting on this event would double-notify and hijack the reading pane.
    track(on<NotifyNewPayload>('notify:new', () => {}));

    track(
      on<OpenThreadPayload>('nav:open-thread', (p) => {
        latest.current.onOpenThread({ accountId: p.accountId, threadId: p.threadId });
      }),
    );

    track(
      on<BadgePayload>('badge:update', (p) => {
        latest.current.onBadge(p.count);
      }),
    );

    return () => {
      disposed = true;
      for (const unlisten of unsubs) unlisten();
      unsubs.length = 0;
    };
  }, []);
}
