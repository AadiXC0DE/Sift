import type { Address, MessageMeta } from '../../app/ipc/types';
import { isValidEmail, parseAddress } from './recipients';

/**
 * What a composer was opened for (P5.4). The reader passes the account, thread
 * and — when the user selected one — the exact message, so the composer never
 * has to guess which message a reply belongs to.
 */
export interface ComposeContext {
  accountId: string;
  threadId: string;
  /** Explicitly selected message; the newest non-draft message otherwise. */
  messageId?: string;
}

export interface ComposeRequest {
  mode: string;
  thread?: ComposeContext;
  /** An existing local draft to reopen instead of prefilling a new one (P5.2). */
  draftId?: string;
}

const RE_SUBJECT = /^\s*re\s*:/i;
const FWD_SUBJECT = /^\s*(fwd|fw)\s*:/i;

/** One prefix, never a doubled or stacked one (P5.4). */
export function prefixedSubject(subject: string, prefix: 'Re' | 'Fwd'): string {
  const s = subject.trim();
  if (!s) return '';
  if (prefix === 'Re' ? RE_SUBJECT.test(s) : FWD_SUBJECT.test(s)) return s;
  return `${prefix}: ${s}`;
}

/** True when `a` is one of the sending account's own addresses. */
export function isIdentity(a: Address | string, identities: string[]): boolean {
  const email = (typeof a === 'string' ? a : a.e).trim().toLowerCase();
  return !!email && identities.some((id) => id.trim().toLowerCase() === email);
}

/**
 * The message a reply/forward is built from: the explicitly selected one when
 * it is still in the thread, otherwise the newest message that is not a draft.
 */
export function pickParent(messages: MessageMeta[], selectedId?: string): MessageMeta | null {
  if (!messages.length) return null;
  if (selectedId) {
    const selected = messages.find((m) => m.id === selectedId);
    if (selected) return selected;
  }
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    if (!messages[i].isDraft) return messages[i];
  }
  return null;
}

function dedupe(list: Address[]): Address[] {
  const seen = new Set<string>();
  const out: Address[] = [];
  for (const a of list) {
    const key = a.e.trim().toLowerCase();
    if (!key || seen.has(key)) continue;
    seen.add(key);
    out.push(a);
  }
  return out;
}

/**
 * Reply addresses for a single reply. Reply-To wins when it is a usable
 * address, else From; a message this account already sent replies to the
 * people it was sent to rather than back to the sender.
 */
export function replyRecipients(parent: MessageMeta, identities: string[]): Address[] {
  if (parent.isSentByMe || isIdentity(parent.from, identities)) {
    const originals = dedupe([...parent.to, ...parent.cc]).filter((a) => !isIdentity(a, identities));
    if (originals.length) return originals;
  }
  const replyTo = parent.replyTo ? parseAddress(parent.replyTo) : null;
  if (replyTo && isValidEmail(replyTo.e)) return [replyTo];
  return [parent.from];
}

/**
 * Reply-all recipients: the reply target plus the original To, then the
 * original Cc. The sending account's own identities and duplicates are
 * removed, and a prior Bcc is never exposed.
 */
export function replyAllRecipients(
  parent: MessageMeta,
  identities: string[],
): { to: Address[]; cc: Address[] } {
  const to: Address[] = [];
  const cc: Address[] = [];
  const seen = new Set<string>();
  const push = (a: Address, list: Address[]) => {
    const key = a.e.trim().toLowerCase();
    if (!key || seen.has(key) || isIdentity(a, identities) || !isValidEmail(a.e)) return;
    seen.add(key);
    list.push(a);
  };
  for (const a of replyRecipients(parent, identities)) push(a, to);
  for (const a of parent.to) push(a, to);
  for (const a of parent.cc) push(a, cc);
  return { to, cc };
}

/** Forward carries no recipients: the user chooses them. */
export function forwardSubject(subject: string): string {
  return prefixedSubject(subject, 'Fwd');
}

export function replySubject(subject: string): string {
  return prefixedSubject(subject, 'Re');
}
