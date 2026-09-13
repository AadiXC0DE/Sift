import { useEffect, useRef, type RefObject } from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';

/** Minimal rectangle in CSS pixels (the DOM's `getBoundingClientRect` shape). */
export interface CssRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * Tauri reports drop points in native physical pixels; DOM rects are CSS
 * pixels. Divide by the scale factor before hit-testing.
 */
export function physicalPointInRect(rect: CssRect, point: { x: number; y: number }, scale: number): boolean {
  const s = scale > 0 ? scale : 1;
  const x = point.x / s;
  const y = point.y / s;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

export function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return cut >= 0 ? path.slice(cut + 1) : path;
}

/** Human-readable reason from a SiftError body (`{code,message,retryable}`) or a string. */
export function errorReason(error: unknown): string {
  if (typeof error === 'string') return error;
  const message = (error as { message?: unknown })?.message;
  return typeof message === 'string' && message ? message : 'unknown error';
}

/**
 * Attach failures must name the file(s) involved: the native command reports a
 * batch-level error, so fold the dropped/picked filenames into the message and
 * avoid repeating a name the backend already included.
 */
export function stagingFailureMessage(paths: string[], error: unknown): string {
  const reason = errorReason(error);
  const names = paths.map(basename);
  if (!names.length) return `Couldn't attach the file: ${reason}`;
  if (names.some((n) => reason.includes(n))) return `Couldn't attach: ${reason}`;
  const list = names.length > 3 ? `${names.slice(0, 3).join(', ')} +${names.length - 3}` : names.join(', ');
  return `Couldn't attach ${list}: ${reason}`;
}

/**
 * Native drag/drop is registered only while the composer is mounted and only
 * for drops whose point lands inside `targetRef`; the listener is removed on
 * close. Browser `File.path` is never consulted (nonstandard and unreliable).
 */
export function useNativeDropListener(
  targetRef: RefObject<HTMLElement | null>,
  onDropPaths: (paths: string[]) => void,
): void {
  const handler = useRef(onDropPaths);
  handler.current = onDropPaths;
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      try {
        const un = await getCurrentWebview().onDragDropEvent((event) => {
          const payload = event.payload;
          if (payload.type !== 'drop' || !payload.paths.length) return;
          const el = targetRef.current;
          if (!el) return;
          const rect = el.getBoundingClientRect();
          if (!physicalPointInRect(rect, payload.position, window.devicePixelRatio || 1)) return;
          handler.current(payload.paths);
        });
        if (disposed) un();
        else unlisten = un;
      } catch {
        /* Browser preview / tests: no native webview to listen to. */
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [targetRef]);
}
