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
