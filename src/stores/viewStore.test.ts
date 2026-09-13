import { describe, it, expect, beforeEach } from 'vitest';
import { useView } from './viewStore';

describe('P1-T11 + P4-T17 pane cycle and unified query', () => {
  beforeEach(() => {
    useView.setState({ paneLayout: 'right', accountScope: 'all' });
  });
  it('` cycles right -> bottom -> off -> right', () => {
    const s = useView.getState();
    expect(s.paneLayout).toBe('right');
    s.cyclePane();
    expect(useView.getState().paneLayout).toBe('bottom');
    s.cyclePane();
    expect(useView.getState().paneLayout).toBe('off');
    s.cyclePane();
    expect(useView.getState().paneLayout).toBe('right');
  });
  it('unified query passes all included accountIds', async () => {
    const { useAccounts } = await import('./accountsStore');
    useAccounts.setState({
      accounts: [
        {
          id: 'a',
          email: 'a@x',
          provider: 'gmail',
          color: 'blue',
          sync_state: 'partial',
          created_at: 0,
          sort_order: 0,
        } as never,
        {
          id: 'b',
          email: 'b@x',
          provider: 'gmail',
          color: 'rose',
          sync_state: 'partial',
          created_at: 0,
          sort_order: 1,
        } as never,
      ],
      included: { a: true, b: false },
    });
    expect(useAccounts.getState().includedIds()).toEqual(['a']);
  });
});

describe('P3.2 account-qualified open thread', () => {
  beforeEach(() => {
    useView.setState({ openThread: null, accountScope: 'all', view: { kind: 'inbox' } });
  });

  it('clears a reader owned by another account when the scope narrows', () => {
    useView.setState({ openThread: { accountId: 'a', threadId: 't1' }, accountScope: 'all' });
    useView.getState().setScope('b');
    expect(useView.getState().openThread).toBeNull();
    // Switching into the owning account keeps it.
    useView.setState({ openThread: { accountId: 'b', threadId: 't2' }, accountScope: 'all' });
    useView.getState().setScope('b');
    expect(useView.getState().openThread).toEqual({ accountId: 'b', threadId: 't2' });
  });

  it('keeps the reader when returning to the unified scope', () => {
    useView.setState({ openThread: { accountId: 'a', threadId: 't1' }, accountScope: 'a' });
    useView.getState().setScope('all');
    expect(useView.getState().openThread).toEqual({ accountId: 'a', threadId: 't1' });
  });

  it('clears the open thread on view navigation', () => {
    useView.setState({ openThread: { accountId: 'a', threadId: 't1' } });
    useView.getState().setView({ kind: 'archive' });
    expect(useView.getState().openThread).toBeNull();
  });
});
