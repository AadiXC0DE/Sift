import type { Address } from '../../app/ipc/types';

// Reply-all computation (P7-T08): reply to sender first, me excluded, dedupe case-insensitive.
export function replyAll(
  from: Address,
  to: Address[],
  cc: Address[],
  me: string,
): { to: Address[]; cc: Address[] } {
  const meL = me.toLowerCase();
  const seen = new Set<string>([meL]);
  const outTo: Address[] = [];
  const push = (a: Address, arr: Address[]) => {
    const k = a.e.toLowerCase();
    if (!k || seen.has(k)) return;
    seen.add(k);
    arr.push(a);
  };
  // sender first
  push(from, outTo);
  for (const a of to) push(a, outTo);
  const outCc: Address[] = [];
  for (const a of cc) push(a, outCc);
  return { to: outTo, cc: outCc };
}
