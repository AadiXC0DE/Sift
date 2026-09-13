import type { Account } from '../../app/ipc/types';

export const LAST_SENDER_KEY = 'sift.compose.lastSender';

/**
 * The sender the user last deliberately composed from. Persisted so a new
 * message in the unified scope does not silently jump back to the first
 * account after a restart.
 */
export function readLastSender(): string | null {
  try {
    return window.localStorage.getItem(LAST_SENDER_KEY);
  } catch {
    return null;
  }
}

export function rememberLastSender(id: string): void {
  try {
    window.localStorage.setItem(LAST_SENDER_KEY, id);
  } catch {
    /* storage disabled: session-only behavior */
  }
}

/**
 * Which account a new message composes from.
 * - A narrowed account scope always wins (you are looking at that account).
 * - Unified scope uses the last deliberately used sender, else the first
 *   enabled account.
 * Replies never call this; they keep their source account.
 */
export function resolveComposeAccount(opts: {
  scope: string | 'all';
  accounts: Pick<Account, 'id'>[];
  enabledIds?: string[];
  lastSender?: string | null;
}): string {
  const ids = opts.accounts.map((a) => a.id);
  if (opts.scope !== 'all' && ids.includes(opts.scope)) return opts.scope;
  const enabled = (opts.enabledIds?.length ? opts.enabledIds : ids).filter((id) => ids.includes(id));
  if (opts.lastSender && enabled.includes(opts.lastSender)) return opts.lastSender;
  return enabled[0] ?? ids[0] ?? '';
}
