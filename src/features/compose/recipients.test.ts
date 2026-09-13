import { describe, it, expect } from 'vitest';
import {
  firstInvalid,
  formatAddress,
  formatAddressList,
  isValidEmail,
  mergeRecipients,
  parseAddress,
  parseAddressList,
  replyAll,
  splitAddressList,
} from './recipients';

describe('P7-T08 reply-all', () => {
  it('excludes me, sender first, dedupes case-insensitively', () => {
    const me = 'me@x.com';
    const from = { e: 'ada@x.com', n: 'Ada' };
    const to = [{ e: 'me@x.com' }, { e: 'ben@y.org' }, { e: 'BEN@y.org' }];
    const cc = [{ e: 'cara@z.org' }, { e: 'me@x.com' }];
    const { to: t, cc: c } = replyAll(from, to, cc, me);
    expect(t.map((x) => x.e)).toEqual(['ada@x.com', 'ben@y.org']);
    expect(c.map((x) => x.e)).toEqual(['cara@z.org']);
  });
});

describe('P5.5 structured address parsing', () => {
  it('does not split a quoted display name containing a comma', () => {
    expect(splitAddressList('"Doe, John" <john@x.com>, ben@y.org')).toEqual([
      '"Doe, John" <john@x.com>',
      'ben@y.org',
    ]);
    expect(parseAddressList('"Doe, John" <john@x.com>, ben@y.org')).toEqual([
      { n: 'Doe, John', e: 'john@x.com' },
      { e: 'ben@y.org' },
    ]);
  });

  it('accepts semicolons as separators', () => {
    expect(parseAddressList('a@x.com; b@y.org').map((a) => a.e)).toEqual(['a@x.com', 'b@y.org']);
  });

  it('parses bare, angled and quoted addresses', () => {
    expect(parseAddress('ada@x.com')).toEqual({ e: 'ada@x.com' });
    expect(parseAddress('Ada Lovelace <ada@x.com>')).toEqual({ n: 'Ada Lovelace', e: 'ada@x.com' });
    expect(parseAddress('"Ada" <ada@x.com>')).toEqual({ n: 'Ada', e: 'ada@x.com' });
    expect(parseAddress('mailto:ada@x.com')).toEqual({ e: 'ada@x.com' });
    expect(parseAddress('   ')).toBeNull();
  });

  it('round-trips a display name with a comma through format and parse', () => {
    const a = { n: 'Doe, John', e: 'john@x.com' };
    expect(formatAddress(a)).toBe('"Doe, John" <john@x.com>');
    expect(parseAddressList(formatAddressList([a, { e: 'b@y.org' }]))).toEqual([a, { e: 'b@y.org' }]);
  });

  it('merges recipients without case-insensitive duplicates', () => {
    const merged = mergeRecipients([{ e: 'A@x.com' }], [{ e: 'a@X.com', n: 'A' }, { e: 'b@y.org' }]);
    expect(merged).toEqual([{ e: 'A@x.com' }, { e: 'b@y.org' }]);
  });

  it('flags invalid chips and accepts ordinary addresses', () => {
    expect(isValidEmail('a@b.c')).toBe(true);
    expect(isValidEmail('foo')).toBe(false);
    expect(isValidEmail('a b@c.d')).toBe(false);
    expect(isValidEmail('a@b@c')).toBe(false);
    expect(firstInvalid([{ e: 'ok@x.com' }])).toBeNull();
    expect(firstInvalid([{ e: 'ok@x.com' }, { e: 'bad' }])).toEqual({ e: 'bad' });
  });
});
