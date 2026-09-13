import { describe, it, expect } from 'vitest';
import { resolveComposeAccount } from './composeAccount';

const accounts = [{ id: 'a1' }, { id: 'a2' }, { id: 'a3' }];

describe('P5.5 new-message account scope', () => {
  it('uses the selected account scope when it still exists', () => {
    expect(resolveComposeAccount({ scope: 'a2', accounts, lastSender: 'a3' })).toBe('a2');
  });

  it('unified scope uses the last deliberately used sender', () => {
    expect(resolveComposeAccount({ scope: 'all', accounts, lastSender: 'a3' })).toBe('a3');
  });

  it('unified scope falls back to the first enabled account', () => {
    expect(resolveComposeAccount({ scope: 'all', accounts, lastSender: null })).toBe('a1');
    expect(resolveComposeAccount({ scope: 'all', accounts, lastSender: 'gone' })).toBe('a1');
  });

  it('skips accounts excluded from the unified scope', () => {
    expect(
      resolveComposeAccount({ scope: 'all', accounts, enabledIds: ['a2', 'a3'], lastSender: 'a1' }),
    ).toBe('a2');
  });

  it('falls back when the narrowed scope no longer exists', () => {
    expect(resolveComposeAccount({ scope: 'gone', accounts, lastSender: 'a2' })).toBe('a2');
  });

  it('returns empty when there is no account to compose from', () => {
    expect(resolveComposeAccount({ scope: 'all', accounts: [] })).toBe('');
  });
});
