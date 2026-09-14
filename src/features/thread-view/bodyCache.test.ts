import { afterEach, describe, expect, it } from 'vitest';
import type { MessageBody } from '../../app/ipc/types';
import {
  MAX_CACHE_BYTES,
  MAX_CACHE_ENTRIES,
  RENDER_VERSION,
  bodyBytes,
  bodyCacheKey,
  bumpPrivacyGeneration,
  cacheGet,
  cacheReset,
  cacheSet,
  cacheState,
  currentPrivacyGeneration,
  notePrivacyGeneration,
  utf8Bytes,
} from './bodyCache';

function body(id: string, text = 'body', overrides: Partial<MessageBody> = {}): MessageBody {
  return {
    messageId: id,
    state: 'ready',
    text,
    remoteImageCount: 0,
    trackerCount: 0,
    darkSafe: true,
    remoteImagesAllowed: false,
    ...overrides,
  };
}

afterEach(() => cacheReset());

describe('P9.2 body cache budget', () => {
  it('counts UTF-8 bytes rather than characters', () => {
    expect(utf8Bytes('abc')).toBe(3);
    expect(utf8Bytes('é')).toBe(2);
    expect(utf8Bytes('日')).toBe(3);
    expect(utf8Bytes('🙂')).toBe(4);
    expect(bodyBytes(body('m', 'é日🙂'))).toBe(2 + 3 + 4);
  });

  it('evicts by entry count', () => {
    for (let i = 0; i < MAX_CACHE_ENTRIES + 5; i++) cacheSet('a', `m${i}`, body(`m${i}`));
    expect(cacheState().count).toBe(MAX_CACHE_ENTRIES);
    expect(cacheGet('a', 'm0')).toBeUndefined();
    expect(cacheGet('a', `m${MAX_CACHE_ENTRIES + 4}`)).toBeDefined();
  });

  it('evicts by total text bytes, not by entry count', () => {
    const big = 'x'.repeat(4 * 1024 * 1024);
    for (const id of ['m1', 'm2', 'm3', 'm4', 'm5']) cacheSet('a', id, body(id, big));
    // Five 4 MiB bodies cannot fit in a 16 MiB budget: the oldest pays for the
    // newest, and the cap is never exceeded.
    expect(cacheState().bytes).toBeLessThanOrEqual(MAX_CACHE_BYTES);
    expect(cacheGet('a', 'm1')).toBeUndefined();
    expect(cacheGet('a', 'm5')).toBeDefined();
    expect(cacheState().count).toBeLessThan(MAX_CACHE_ENTRIES);
  });

  it('never caches a body larger than the whole budget', () => {
    cacheSet('a', 'huge', body('huge', 'y'.repeat(MAX_CACHE_BYTES + 1)));
    expect(cacheGet('a', 'huge')).toBeUndefined();
    expect(cacheState().bytes).toBe(0);
  });

  it('keeps the least recently used entry, not the oldest inserted', () => {
    cacheSet('a', 'm1', body('m1'));
    cacheSet('a', 'm2', body('m2'));
    cacheSet('a', 'm3', body('m3'));
    // Reading m1 makes it the most recently used.
    expect(cacheGet('a', 'm1')).toBeDefined();
    for (let i = 4; i <= MAX_CACHE_ENTRIES; i++) cacheSet('a', `m${i}`, body(`m${i}`));
    cacheSet('a', 'm-new', body('m-new'));
    expect(cacheGet('a', 'm1')).toBeDefined();
    expect(cacheGet('a', 'm2')).toBeUndefined();
  });
});

describe('P9.2 body cache identity', () => {
  it('separates the same message id in two accounts', () => {
    cacheSet('a', 'dup', body('dup', 'account a'));
    cacheSet('b', 'dup', body('dup', 'account b'));
    expect(cacheGet('a', 'dup')?.text).toBe('account a');
    expect(cacheGet('b', 'dup')?.text).toBe('account b');
  });

  it('keys on the render version and the privacy generation', () => {
    cacheSet('a', 'm1', body('m1'));
    expect(cacheGet('a', 'm1')).toBeDefined();
    expect(bodyCacheKey('a', 'm1', RENDER_VERSION + 1, currentPrivacyGeneration())).not.toBe(
      bodyCacheKey('a', 'm1'),
    );
  });

  it('drops every cached body when the remote-content policy changes', () => {
    cacheSet('a', 'm1', body('m1'));
    const before = currentPrivacyGeneration();
    bumpPrivacyGeneration();
    expect(currentPrivacyGeneration()).toBe(before + 1);
    expect(cacheGet('a', 'm1')).toBeUndefined();
    expect(cacheState().count).toBe(0);
  });

  it('adopts a newer backend generation but ignores stale and invalid ones', () => {
    notePrivacyGeneration(7);
    expect(currentPrivacyGeneration()).toBe(7);
    cacheSet('a', 'm1', body('m1'));
    notePrivacyGeneration(3);
    notePrivacyGeneration(undefined);
    notePrivacyGeneration(Number.NaN);
    expect(currentPrivacyGeneration()).toBe(7);
    expect(cacheGet('a', 'm1')).toBeDefined();
    notePrivacyGeneration(9);
    expect(cacheGet('a', 'm1')).toBeUndefined();
  });
});

describe('P9.2 body cache honesty', () => {
  it('refuses to store a retryable failure', () => {
    cacheSet('a', 'm1', body('m1', 'offline', { state: 'loading' }));
    expect(cacheGet('a', 'm1')).toBeUndefined();
  });

  it('refuses to store an error body', () => {
    cacheSet('a', 'm1', body('m1', 'provider said no', { state: 'error' }));
    expect(cacheGet('a', 'm1')).toBeUndefined();
    expect(cacheState().count).toBe(0);
  });

  it('replaces an existing entry with the same key instead of double counting', () => {
    cacheSet('a', 'm1', body('m1', 'first'));
    cacheSet('a', 'm1', body('m1', 'second'));
    expect(cacheState().count).toBe(1);
    expect(cacheState().bytes).toBe(bodyBytes(body('m1', 'second')));
    expect(cacheGet('a', 'm1')?.text).toBe('second');
  });
});
