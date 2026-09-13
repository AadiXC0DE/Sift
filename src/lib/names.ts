import type { Address } from '../app/ipc/types';

export function firstName(a: Address): string {
  if (a.n) return a.n.split(' ')[0];
  return a.e.split('@')[0];
}

export function participantsLabel(parts: Address[], count: number): string {
  if (parts.length === 0) return '(No sender)';
  if (parts.length === 1 && count <= 1) {
    const p = parts[0];
    return p.n ?? p.e.split('@')[0];
  }
  const names = parts.slice(0, 6).map(firstName);
  // put me last
  names.sort((x, y) => (x === 'me' ? 1 : 0) - (y === 'me' ? 1 : 0));
  return count > 1 ? `${names.join(', ')} ${count}` : names.join(', ');
}

/**
 * Headline for a conversation the reader sent (P3.6).
 *
 * A sent row's `participants` are the people in the conversation, and the first
 * of them is the account that sent it — the reader themselves, which says
 * nothing about who received it. Addresses belonging to `selfEmails` are
 * dropped so the row names the recipients instead. Returns `null` when the
 * conversation holds nobody else (a note to self), and the caller falls back to
 * the ordinary participant label rather than inventing a recipient.
 */
export function recipientsLabel(parts: Address[], selfEmails: string[]): string | null {
  const self = new Set(selfEmails.filter(Boolean).map((e) => e.toLowerCase()));
  const to = parts.filter((p) => !self.has(p.e.toLowerCase()));
  if (to.length === 0) return null;
  const names = to.slice(0, 6).map(firstName);
  return `To ${names.join(', ')}${to.length > 6 ? '…' : ''}`;
}

export function initials(email: string, name?: string | null): string {
  if (name) {
    const ps = name.split(' ').filter(Boolean);
    if (ps.length >= 2) return (ps[0][0] + ps[1][0]).toUpperCase();
    return name.slice(0, 2).toUpperCase();
  }
  return email.slice(0, 2).toUpperCase();
}

export function avatarHue(email: string): number {
  let h = 0;
  for (const c of email) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}
