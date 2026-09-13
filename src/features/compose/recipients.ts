import type { Address } from '../../app/ipc/types';

/**
 * Split an address list on the separators (`,` / `;`) that sit outside double
 * quotes and angle brackets, so `"Doe, John" <j@x.com>, a@b.com` is two
 * addresses rather than three fragments. Quotes may be escaped with `\`.
 */
export function splitAddressList(raw: string): string[] {
  const out: string[] = [];
  let cur = '';
  let inQuote = false;
  let inAngle = false;
  let escaped = false;
  for (const ch of raw) {
    if (escaped) {
      cur += ch;
      escaped = false;
      continue;
    }
    if (ch === '\\' && inQuote) {
      escaped = true;
      cur += ch;
      continue;
    }
    if (ch === '"') inQuote = !inQuote;
    else if (ch === '<' && !inQuote) inAngle = true;
    else if (ch === '>' && inAngle) inAngle = false;

    if (!inQuote && !inAngle && (ch === ',' || ch === ';')) {
      out.push(cur);
      cur = '';
      continue;
    }
    cur += ch;
  }
  out.push(cur);
  return out.map((s) => s.trim()).filter(Boolean);
}

/** Strip surrounding quotes and unescape `\"` / `\\`. */
function unquote(raw: string): string {
  const t = raw.trim();
  if (t.length >= 2 && t.startsWith('"') && t.endsWith('"')) {
    return t.slice(1, -1).replace(/\\(["\\])/g, '$1');
  }
  return t;
}

/**
 * Bare address check: exactly one `@` with content on both sides and no
 * whitespace or list separator anywhere. Used by chip blur and by send
 * validation, which must agree on what "valid" means.
 */
export function isValidEmail(email: string): boolean {
  const e = email.trim();
  if (!e || /[\s,;]/.test(e)) return false;
  const at = e.indexOf('@');
  return at > 0 && at === e.lastIndexOf('@') && at < e.length - 1;
}

/**
 * Parse one address token. Accepts `Name <a@b>`, `"Doe, John" <a@b>`, a bare
 * address, and a quoted address. Returns null for empty or unusable input.
 */
export function parseAddress(token: string): Address | null {
  const raw = token.trim();
  if (!raw) return null;
  const m = raw.match(/^(.*)<([^>]*)>$/s);
  if (m) {
    const n = unquote(m[1]);
    const e = unquote(m[2]).replace(/^mailto:/i, '');
    if (!e) return null;
    return n && n !== e ? { n, e } : { e };
  }
  const bare = unquote(raw).replace(/^mailto:/i, '');
  return bare ? { e: bare } : null;
}

/** Parse a free-form address list (chip commit, paste, blur). */
export function parseAddressList(raw: string): Address[] {
  return splitAddressList(raw)
    .map(parseAddress)
    .filter((a): a is Address => a !== null);
}

/** Serialize one address for the list input. Display names are always quoted
 * so a comma inside them can never split the list again. */
export function formatAddress(a: Address): string {
  if (!a.n) return a.e;
  return `"${a.n.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}" <${a.e}>`;
}

export function formatAddressList(list: Address[]): string {
  return list.map(formatAddress).join(', ');
}

/** Append recipients, dropping case-insensitive duplicates of existing ones. */
export function mergeRecipients(existing: Address[], incoming: Address[]): Address[] {
  const out = [...existing];
  for (const a of incoming) {
    if (!out.some((x) => x.e.trim().toLowerCase() === a.e.trim().toLowerCase())) out.push(a);
  }
  return out;
}

/** Validate every chip; returns the first invalid one so the caller can name it. */
export function firstInvalid(list: Address[]): Address | null {
  return list.find((a) => !isValidEmail(a.e)) ?? null;
}

// Reply-all computation (P7-T08): reply to sender first, me excluded, dedupe case-insensitive.
export function replyAll(
  from: Address,
  to: Address[],
  cc: Address[],
  me: string,
): { to: Address[]; cc: Address[] } {
  const seen = new Set<string>([me.toLowerCase()]);
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
