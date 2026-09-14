import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ThreadRow } from '../../app/ipc/types';
import { useSelection } from '../../stores/selectionStore';
import { useView } from '../../stores/viewStore';
import { allCommands } from '../palette/registry';
import {
  groupKeysByAccount,
  hasListContext,
  resolveCommandTargets,
  runMailCommand,
  setListContext,
} from './listCommands';

const threadsAction = vi.fn(async () => ({ gestureId: 'g1', operations: [] }));

vi.mock('../../app/ipc/commands', () => ({
  api: {
    threads_action: (...args: unknown[]) => threadsAction(...(args as [])),
  },
}));
vi.mock('sonner', () => ({ toast: vi.fn(), toastError: vi.fn() }));

function row(accountId: string, id: string, starred = false): ThreadRow {
  return {
    accountId,
    id,
    subject: id,
    snippet: '',
    participants: [],
    lastMessageAt: 0,
    messageCount: 1,
    unreadCount: 1,
    isStarred: starred,
    hasAttachments: false,
    labelIds: [],
  };
}

const rows = [row('a', 't1'), row('a', 't2'), row('b', 't1')];

function reset() {
  useSelection.setState({ focusedKey: null, selectedIds: new Set(), anchorKey: null });
  useView.setState({ openThread: null, accountScope: 'all' });
}

describe('P3.5 command target resolver', () => {
  beforeEach(() => {
    reset();
    setListContext({ rows, openPicker: vi.fn() });
  });

  it('a non-empty selection wins over the focused row', async () => {
    useSelection.setState({ selectedIds: new Set(['a:t1']), focusedKey: 'a:t2' });
    expect(resolveCommandTargets('list')).toEqual([{ accountId: 'a', threadIds: ['t1'] }]);
    await runMailCommand('archive', 'list');
    expect(threadsAction).toHaveBeenCalledWith({
      gestureId: null,
      targets: [{ accountId: 'a', threadId: 't1' }],
      action: { kind: 'archive' },
    });
  });

  it('falls back to the focused row, or the open reader in thread scope', () => {
    useSelection.setState({ focusedKey: 'a:t2' });
    expect(resolveCommandTargets('list')).toEqual([{ accountId: 'a', threadIds: ['t2'] }]);
    // The reader was opened from account b; the same id exists in account a.
    useView.setState({ openThread: { accountId: 'b', threadId: 't1' } });
    expect(resolveCommandTargets('thread')).toEqual([{ accountId: 'b', threadIds: ['t1'] }]);
    // List focus stays the list target even with a reader open.
    expect(resolveCommandTargets('list')).toEqual([{ accountId: 'a', threadIds: ['t2'] }]);
    // Duplicate id: focus on a:t1 must not retarget the reader opened at b:t1.
    useSelection.setState({ focusedKey: 'a:t1' });
    expect(resolveCommandTargets('thread')).toEqual([{ accountId: 'b', threadIds: ['t1'] }]);
    expect(resolveCommandTargets('list')).toEqual([{ accountId: 'a', threadIds: ['t1'] }]);
  });

  it('sends a cross-account selection as one gesture with qualified targets', async () => {
    threadsAction.mockClear();
    useSelection.setState({ selectedIds: new Set(['a:t1', 'b:t1']) });
    await runMailCommand('archive', 'list');
    expect(threadsAction).toHaveBeenCalledTimes(1);
    expect(threadsAction).toHaveBeenCalledWith({
      gestureId: null,
      targets: [
        { accountId: 'a', threadId: 't1' },
        { accountId: 'b', threadId: 't1' },
      ],
      action: { kind: 'archive' },
    });
  });

  it('groups a bulk selection per account so one gesture is one group each', () => {
    expect(groupKeysByAccount(['a:1', 'b:2', 'a:3'])).toEqual([
      { accountId: 'a', threadIds: ['1', '3'] },
      { accountId: 'b', threadIds: ['2'] },
    ]);
  });

  it('star state comes from the resolved row, not a stale reader', async () => {
    useSelection.setState({ selectedIds: new Set(['a:t2']), focusedKey: 'a:t1' });
    await runMailCommand('star', 'list');
    expect(threadsAction).toHaveBeenCalledWith({
      gestureId: null,
      targets: [{ accountId: 'a', threadId: 't2' }],
      action: { kind: 'star', on: true },
    });
  });

  it('exposes the configurable list commands in the central registry', () => {
    useSelection.setState({ selectedIds: new Set(['a:t1']) });
    const ids = allCommands().map((c) => c.id);
    expect(ids).toContain('list-archive');
    expect(ids).toContain('list-trash');
    expect(ids).toContain('list-mark-read');
    expect(ids).toContain('list-clear-selection');
    expect(hasListContext()).toBe(true);
  });
});
