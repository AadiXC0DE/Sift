// Runs background work (chunk prefetch) after the browser has painted and gone
// idle. WebKit has no requestIdleCallback, so fall back to a short timer.
export function onIdle(fn: () => void, timeoutMs = 2000): () => void {
  if (typeof window.requestIdleCallback === 'function') {
    const id = window.requestIdleCallback(fn, { timeout: timeoutMs });
    return () => window.cancelIdleCallback(id);
  }
  const id = window.setTimeout(fn, 200);
  return () => window.clearTimeout(id);
}
