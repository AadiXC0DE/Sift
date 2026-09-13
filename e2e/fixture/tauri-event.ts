/**
 * Test-only replacement for `@tauri-apps/api/event`.
 *
 * Emits go through the shared fixture bus, so `store:threads`/`store:labels`
 * events travel the same path the Rust runtime uses and the frontend's real
 * refresh logic is exercised after every fixture mutation.
 */
import { listen as busListen } from './bus';

export interface Event<T> {
  event: string;
  id: number;
  payload: T;
}

export type UnlistenFn = () => void;

let nextId = 1;

export async function listen<T>(event: string, handler: (e: Event<T>) => void): Promise<UnlistenFn> {
  const id = nextId++;
  const off = busListen(event, (payload) => handler({ event, id, payload: payload as T }));
  return () => {
    off();
  };
}

export async function once<T>(event: string, handler: (e: Event<T>) => void): Promise<UnlistenFn> {
  let off = () => {};
  off = await listen<T>(event, (e) => {
    off();
    handler(e);
  });
  return off;
}
