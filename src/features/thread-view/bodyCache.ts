import type { MessageBody } from '../../app/ipc/types';

/**
 * Bounded body cache for the reader (P9.2).
 *
 * The previous cache was a `Map` capped at 50 *entries* and keyed by
 * `accountId:messageId`, which let a single 2 MiB newsletter occupy the whole
 * budget and let a stale body survive a privacy-policy change or a renderer
 * upgrade. Three properties are load-bearing here:
 *
 * 1. **Identity** — the key carries account, message, render version and the
 *    privacy generation, so a body can never be shown under another message's
 *    id, after the sanitizer changed, or after the user changed which remote
 *    content is allowed.
 * 2. **Budget** — total UTF-8 bytes of cached html+text (16 MiB) *and* entry
 *    count (50); the least recently *used* entry is evicted, not the oldest
 *    inserted one.
 * 3. **Honesty** — only a terminal `ready` body is stored. A retryable
 *    failure is never frozen into the cache as immutable content, so reopening
 *    an old error after reconnect always retries.
 */

/** Bump when the rendered-body contract changes: a body produced by an older
 * sanitizer/CID pipeline must not be reused after an upgrade. */
export const RENDER_VERSION = 2;

export const MAX_CACHE_ENTRIES = 50;
export const MAX_CACHE_BYTES = 16 * 1024 * 1024;

interface Entry {
  body: MessageBody;
  bytes: number;
}

/** Insertion order is the LRU order: a read re-inserts the key at the tail. */
const entries = new Map<string, Entry>();
let cachedBytes = 0;
let privacyGeneration = 0;

/**
 * UTF-8 byte length without allocating an encoded copy — the budget must be
 * measurable while scrolling a 200-message conversation, not after it.
 */
export function utf8Bytes(text: string): number {
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code < 0x80) bytes += 1;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff) {
      // Astral plane: the pair is four bytes. A lone surrogate is counted as
      // three, which is what it costs to carry, never fewer.
      bytes += 4;
      i += 1;
    } else bytes += 3;
  }
  return bytes;
}

export function bodyBytes(body: Pick<MessageBody, 'html' | 'text'>): number {
  return utf8Bytes(body.html ?? '') + utf8Bytes(body.text ?? '');
}

export function bodyCacheKey(
  accountId: string,
  messageId: string,
  renderVersion: number = RENDER_VERSION,
  generation: number = privacyGeneration,
): string {
  // NUL cannot appear in a provider id or account id, so the parts cannot be
  // re-split into a different (account, message) pair.
  return `${accountId}\u0000${messageId}\u0000${renderVersion}\u0000${generation}`;
}

export function currentPrivacyGeneration(): number {
  return privacyGeneration;
}

function purge(): void {
  entries.clear();
  cachedBytes = 0;
}

/**
 * Adopt the backend's permission generation. `settingsStore` calls this
 * whenever the effective remote-content policy changes, and a `MessageBody`
 * that reports a newer generation does too. Entries keyed to the old
 * generation are unreachable, so they are dropped rather than leaked.
 */
export function notePrivacyGeneration(generation: number | undefined | null): void {
  if (typeof generation !== 'number' || !Number.isFinite(generation)) return;
  if (generation <= privacyGeneration) return;
  privacyGeneration = generation;
  purge();
}

/** Invalidate every cached body after a local policy change. */
export function bumpPrivacyGeneration(): void {
  privacyGeneration += 1;
  purge();
}

/**
 * Read a body, marking it most recently used. Returns `undefined` for anything
 * that is not a terminal `ready` body: `loading` is retryable by definition and
 * `error` must be re-attempted when the message is opened again.
 */
export function cacheGet(accountId: string, messageId: string): MessageBody | undefined {
  const key = bodyCacheKey(accountId, messageId);
  const entry = entries.get(key);
  if (!entry) return undefined;
  entries.delete(key);
  entries.set(key, entry);
  return entry.body;
}

/**
 * Store a terminal `ready` body. Anything else is refused: an error or a
 * transient failure is never promoted to cacheable content.
 */
export function cacheSet(accountId: string, messageId: string, body: MessageBody): void {
  if (body.state !== 'ready') return;
  const key = bodyCacheKey(accountId, messageId);
  const bytes = bodyBytes(body);
  const previous = entries.get(key);
  if (previous) {
    cachedBytes -= previous.bytes;
    entries.delete(key);
  }
  // A single body larger than the whole budget is never cached; caching it
  // would evict every other message and still exceed the cap.
  if (bytes > MAX_CACHE_BYTES) return;
  entries.set(key, { body, bytes });
  cachedBytes += bytes;
  evict();
}

function evict(): void {
  while (entries.size > MAX_CACHE_ENTRIES || cachedBytes > MAX_CACHE_BYTES) {
    const oldest = entries.keys().next().value;
    if (oldest === undefined) return;
    const entry = entries.get(oldest);
    if (entry) cachedBytes -= entry.bytes;
    entries.delete(oldest);
  }
}

/** Test/diagnostic view of the budget. */
export function cacheState(): { count: number; bytes: number; generation: number } {
  return { count: entries.size, bytes: cachedBytes, generation: privacyGeneration };
}

/** Test-only reset so the module-level budget cannot leak between tests. */
export function cacheReset(): void {
  purge();
  privacyGeneration = 0;
}
