import { describe, it, expect } from 'vitest';
import { participantsLabel } from './names';

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
