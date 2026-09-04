import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type Handler<T = unknown> = (payload: T) => void;

export async function on<T>(event: string, handler: Handler<T>): Promise<UnlistenFn> {
  return listen<T>(event, (e) => handler(e.payload));
}

export const SiftEvents = [
  'store:threads',
  'store:labels',
  'store:bodies',
  'store:drafts',
  'sync:state',
  'outbox:state',
  'auth:expired',
  'notify:new-mail',
  'snooze:woke',
  'update:available',
  'net:state',
] as const;
