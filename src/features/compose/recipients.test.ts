import { describe, it, expect } from 'vitest';
import { replyAll } from './recipients';

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
