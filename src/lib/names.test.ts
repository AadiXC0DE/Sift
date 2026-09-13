import { describe, it, expect } from 'vitest';
import { participantsLabel, recipientsLabel } from './names';

describe('P4-T02 participants', () => {
  it('Ada, Ben, me 3', () => {
    expect(
      participantsLabel(
        [
          { n: 'Ada', e: 'a@x' },
          { n: 'Ben', e: 'b@x' },
          { e: 'me@x', me: true },
        ],
        3,
      ),
    ).toMatch(/Ada.*Ben.*3/);
  });
  it('single sender full name', () => {
    expect(participantsLabel([{ n: 'Ada Lovelace', e: 'a@x' }], 1)).toBe('Ada Lovelace');
  });
  it('unknown name -> local-part', () => {
    expect(participantsLabel([{ e: 'foo@x.com' }], 1)).toBe('foo');
  });
});

describe('P3.6 sent rows name the recipients', () => {
  it('drops the sending account and keeps the people it was sent to', () => {
    expect(
      recipientsLabel(
        [
          { n: 'Ada Lovelace', e: 'ada@x', me: true },
          { n: 'Client Team', e: 'client@x' },
        ],
        ['ada@x'],
      ),
    ).toBe('To Client');
  });

  it('matches the account address case-insensitively', () => {
    expect(recipientsLabel([{ e: 'Ada@X' }, { e: 'bob@x' }], ['ada@x'])).toBe('To bob');
  });

  it('returns null for a conversation that holds nobody but the reader', () => {
    expect(recipientsLabel([{ n: 'Ada', e: 'ada@x' }], ['ada@x'])).toBeNull();
    expect(recipientsLabel([], ['ada@x'])).toBeNull();
  });
});
