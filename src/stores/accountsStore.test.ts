import { describe, it, expect } from 'vitest';
import { useAccounts } from './accountsStore';

describe('P9-T06 unified scope', () => {
  it('includes only checked; cmd0=all, cmd2=second', () => {
    useAccounts.setState({
      accounts: [
        {
          id: 'a',
          email: 'a@x',
          provider: 'gmail',
          color: 'blue',
          sync_state: 'p',
          created_at: 0,
          sort_order: 0,
        },
        {
          id: 'b',
          email: 'b@x',
          provider: 'gmail',
          color: 'rose',
          sync_state: 'p',
          created_at: 0,
          sort_order: 1,
        },
      ] as never,
      included: { a: true, b: false },
    });
    expect(useAccounts.getState().includedIds()).toEqual(['a']);
  });
});
