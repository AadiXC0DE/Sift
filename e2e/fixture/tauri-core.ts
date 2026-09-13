/**
 * Test-only replacement for `@tauri-apps/api/core`.
 *
 * `e2e/vite.e2e.config.ts` aliases the real module here, so the production
 * bundle is untouched and every `invoke` in the real frontend reaches the
 * fixture backend. Unknown commands throw and are recorded, so a test can never
 * pass because a command silently did nothing.
 */
import { installControl, invokeFixture, type Args } from './backend';

installControl();

export async function invoke<T>(cmd: string, args?: Args): Promise<T> {
  return invokeFixture(cmd, args ?? {}) as Promise<T>;
}

export function isTauri(): boolean {
  return false;
}

/** Mirror of the Tauri Channel API; the fixture delivers each message once. */
export class Channel<T = unknown> {
  onmessage: (message: T) => void = () => {};

  toJSON(): string {
    return 'channel';
  }
}
