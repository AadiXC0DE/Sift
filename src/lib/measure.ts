export function mark(name: string): void {
  try {
    performance.mark(name);
  } catch {
    /* noop */
  }
}
export function measurePerf(name: string, fn: () => void): number {
  const t0 = performance.now();
  fn();
  return performance.now() - t0;
}
