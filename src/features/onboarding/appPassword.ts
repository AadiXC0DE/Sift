/** App-password normalize/validate + display (Step C, P11-T16).
 * Valid when exactly 16 ASCII letters after stripping spaces/dashes.
 * Display groups as 4×4 for readability; validation messages use exact copy.
 */

export function normalizeAppPassword(raw: string): string {
  return raw.replace(/[\s\-‐‑‒–—― 　]+/g, '').toLowerCase();
}

export function isValidAppPassword(raw: string): boolean {
  const clean = normalizeAppPassword(raw);
  return clean.length === 16 && /^[a-z]{16}$/.test(clean);
}

export function appPasswordError(raw: string): string | null {
  const clean = normalizeAppPassword(raw);
  if (clean.length === 0) return null;
  if (clean.length < 16) return 'That password should be 16 letters. Check for a missing character.';
  if (clean.length > 16) return 'That password should be 16 letters. You may have pasted extra characters.';
  if (!/^[a-z]{16}$/.test(clean))
    return 'App passwords use letters only — no numbers. Create a fresh one and paste it again.';
  return null;
}

/** Display grouping: "abcdefghijklmnop" → "abcd efgh ijkl mnop". */
export function groupAppPassword(cleanOrRaw: string): string {
  const clean = normalizeAppPassword(cleanOrRaw).slice(0, 16);
  return [clean.slice(0, 4), clean.slice(4, 8), clean.slice(8, 12), clean.slice(12, 16)]
    .filter(Boolean)
    .join(' ');
}

/** Clipboard assist match: 4×4 letters with optional spaces (case-insensitive). */
export function looksLikeAppPassword(text: string): boolean {
  return /^[a-z]{4}( ?[a-z]{4}){3}$/i.test(text.trim());
}
