/**
 * Event bus shared by the fixture backend and the aliased Tauri event module.
 * Kept free of any app or Tauri import so `src/test/fixture-contract.test.ts`
 * can exercise it under Vitest.
 */
export type FixtureListener = (payload: unknown) => void;

const listeners = new Map<string, Set<FixtureListener>>();

export function emit(event: string, payload: unknown): void {
  const set = listeners.get(event);
  if (!set) return;
  for (const fn of [...set]) fn(payload);
}

export function listen(event: string, handler: FixtureListener): () => void {
  const set = listeners.get(event) ?? new Set<FixtureListener>();
  set.add(handler);
  listeners.set(event, set);
  return () => {
    set.delete(handler);
  };
}

export function listenerCount(event: string): number {
  return listeners.get(event)?.size ?? 0;
}
