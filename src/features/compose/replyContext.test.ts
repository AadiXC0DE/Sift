import { describe, it, expect } from 'vitest';
import {
  forwardSubject,
  pickParent,
  replyAllRecipients,
  replyRecipients,
  replySubject,
} from './replyContext';
import type { Address, MessageMeta } from '../../app/ipc/types';

function message(overrides: Partial<MessageMeta> = {}): MessageMeta {
  return {
    id: 'm1',
    internalDate: 1_760_000_000_000,
    from: { e: 'boss@x.com', n: 'Boss' },
    to: [{ e: 'me@x.com' }],
    cc: [],
    bcc: [],
    subject: 'Quarterly report',
    snippet: 'Please review',
    isUnread: false,
    isStarred: false,
    isDraft: false,
    isSentByMe: false,
    labelIds: [],
    hasAttachments: false,
    attachments: [],
    bodyState: 'fetched',
    ...overrides,
  };
}

const ME = ['me@x.com'];

describe('P5.4 default parent', () => {
  it('uses the explicitly selected message when it belongs to the thread', () => {
    const older = message({ id: 'm1' });
    const newest = message({ id: 'm2' });
    expect(pickParent([older, newest], 'm1')?.id).toBe('m1');
  });

  it('falls back to the newest non-draft message', () => {
    const older = message({ id: 'm1' });
    const draft = message({ id: 'd1', isDraft: true });
    expect(pickParent([older, draft], undefined)?.id).toBe('m1');
    expect(pickParent([older, draft], 'gone')?.id).toBe('m1');
  });

  it('has no parent for an empty thread', () => {
    expect(pickParent([])).toBeNull();
  });
});

describe('P5.4 reply recipient', () => {
  it('prefers a valid Reply-To over From', () => {
    const m = message({ replyTo: 'help@lists.example' });
    expect(replyRecipients(m, ME).map((a) => a.e)).toEqual(['help@lists.example']);
  });

  it('keeps the display name of a Reply-To that carries one', () => {
    const m = message({ replyTo: 'Support <support@example.test>' });
    expect(replyRecipients(m, ME)).toEqual([{ e: 'support@example.test', n: 'Support' }]);
  });

  it('ignores an unusable Reply-To and uses From', () => {
    const m = message({ replyTo: 'not an address' });
    expect(replyRecipients(m, ME).map((a) => a.e)).toEqual(['boss@x.com']);
  });

  it('replying to your own sent message targets the original recipients', () => {
    const m = message({
      isSentByMe: true,
      from: { e: 'me@x.com' },
      to: [{ e: 'ada@x.com' }, { e: 'ben@y.org' }],
      cc: [{ e: 'cara@z.org' }],
    });
    const to = replyRecipients(m, ME).map((a) => a.e);
    expect(to).toEqual(['ada@x.com', 'ben@y.org', 'cara@z.org']);
    expect(to).not.toContain('me@x.com');
  });

  it('recognizes own addresses by identity, not just the flag', () => {
    const m = message({
      from: { e: 'Me@X.com' },
      to: [{ e: 'ada@x.com' }],
      cc: [],
    });
    expect(replyRecipients(m, ME).map((a) => a.e)).toEqual(['ada@x.com']);
  });

  it('an own message with no other recipient falls back to From', () => {
    const m = message({ isSentByMe: true, from: { e: 'me@x.com' }, to: [{ e: 'me@x.com' }], cc: [] });
    expect(replyRecipients(m, ME).map((a) => a.e)).toEqual(['me@x.com']);
  });
});

describe('P5.4 reply all', () => {
  it('builds To from the reply target plus the original To, and Cc from the original Cc', () => {
    const m = message({
      from: { e: 'boss@x.com' },
      to: [{ e: 'me@x.com' }, { e: 'ada@x.com' }],
      cc: [{ e: 'cara@z.org' }],
    });
    const { to, cc } = replyAllRecipients(m, ME);
    expect(to.map((a) => a.e)).toEqual(['boss@x.com', 'ada@x.com']);
    expect(cc.map((a) => a.e)).toEqual(['cara@z.org']);
  });

  it('removes every one of the account identities and duplicate addresses', () => {
    const aliases = ['me@x.com', 'alias@x.com'];
    const m = message({
      from: { e: 'boss@x.com' },
      to: [{ e: 'alias@x.com' }, { e: 'Boss@X.com' }, { e: 'me@x.com' }, { e: 'ada@x.com' }],
      cc: [{ e: 'ada@x.com' }, { e: 'cara@z.org' }, { e: 'me@x.com' }],
    });
    const { to, cc } = replyAllRecipients(m, aliases);
    expect(to.map((a) => a.e)).toEqual(['boss@x.com', 'ada@x.com']);
    expect(cc.map((a) => a.e)).toEqual(['cara@z.org']);
  });

  it('never exposes a prior Bcc', () => {
    const m = message({
      from: { e: 'boss@x.com' },
      to: [{ e: 'ada@x.com' }],
      cc: [],
      bcc: [{ e: 'secret@x.com' }],
    });
    const { to, cc } = replyAllRecipients(m, ME);
    expect([...to, ...cc].map((a) => a.e)).not.toContain('secret@x.com');
  });

  it('skips malformed addresses instead of sending to them', () => {
    const m = message({
      to: [{ e: 'ada@x.com' }, { e: 'broken' } as Address],
      cc: [],
    });
    expect(replyAllRecipients(m, ME).to.map((a) => a.e)).toEqual(['boss@x.com', 'ada@x.com']);
  });
});

describe('P5.4 subject prefixes', () => {
  it('adds exactly one prefix', () => {
    expect(replySubject('Quarterly report')).toBe('Re: Quarterly report');
    expect(replySubject('Re: Quarterly report')).toBe('Re: Quarterly report');
    expect(replySubject('RE: Quarterly report')).toBe('RE: Quarterly report');
    expect(forwardSubject('Quarterly report')).toBe('Fwd: Quarterly report');
    expect(forwardSubject('Fwd: Quarterly report')).toBe('Fwd: Quarterly report');
  });

  it('keeps an existing chain when the prefix is different', () => {
    expect(forwardSubject('Re: Quarterly report')).toBe('Fwd: Re: Quarterly report');
    expect(replySubject('Fwd: Quarterly report')).toBe('Re: Fwd: Quarterly report');
  });

  it('leaves an empty subject empty rather than inventing "Re:"', () => {
    expect(replySubject('   ')).toBe('');
    expect(forwardSubject('')).toBe('');
  });
});
