/**
 * Source-view helpers (P9.3).
 *
 * The backend hands the frontend a decoded string, never the bytes that came
 * off the wire. A message whose body is not valid UTF-8 therefore arrives with
 * U+FFFD replacement characters, and a NUL byte in the source survives
 * decoding untouched. Both facts are what the viewer's caveat and
 * `looksBinary` are built on, so the dialog never promises a byte-exact view
 * it cannot deliver.
 */

/**
 * Split a raw source into display lines.
 *
 * Only `\n` splits: a CR that arrived with the message stays on its line, so
 * `sourceLines(raw).join('\n') === raw` and the view shows what was decoded
 * instead of quietly normalising line endings.
 */
export function sourceLines(raw: string): string[] {
  return raw.split('\n');
}

/**
 * True when decoding already lost information: a U+FFFD replacement character
 * or a NUL byte means the source was not clean text, so what the viewer shows
 * is an approximation of the original bytes.
 */
export function looksBinary(raw: string): boolean {
  return raw.includes('\uFFFD') || raw.includes('\u0000');
}
