/** Plain-text composition helpers (P5.5 plain-text toggle). */

/** Convert pasted HTML to readable text without executing or keeping markup. */
export function htmlToPlainText(html: string): string {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  return doc.body.textContent ?? '';
}

/** Escape text so it survives an HTML paste pipeline as literal characters. */
export function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}
