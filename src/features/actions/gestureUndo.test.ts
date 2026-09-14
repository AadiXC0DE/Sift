import { describe, it, expect, vi, beforeEach } from 'vitest';

const mocks = vi.hoisted(() => ({
  api: { threads_action: vi.fn(), action_undo: vi.fn(), snooze_set: vi.fn(), snooze_clear: vi.fn() },
  toast: Object.assign(vi.fn(), { error: vi.fn(), success: vi.fn() }),
}));

vi.mock('../../app/ipc/commands', () => ({ api: mocks.api }));
vi.mock('sonner', () => ({ toast: mocks.toast }));
vi.mock('../../stores/settingsStore', () => ({
  useSettings: Object.assign(
    (sel: (s: unknown) => unknown) => sel({ settings: { undoToastDuration: 8, wakeSnoozedUnread: true } }),
    { getState: () => ({ settings: { undoToastDuration: 8, wakeSnoozedUnread: true } }) },
  ),
}));

import { dispatchGesture, registerUndo, undoGesture, undoLast, undoGroups } from './dispatch';
import { snoozeTargets, unsnoozeTargets } from '../snooze/snoozeActions';
import { readSiftError } from '../../lib/siftError';

beforeEach(() => {
  vi.clearAllMocks();
  undoGroups().length = 0;
  mocks.api.threads_action.mockResolvedValue({ gestureId: 'g-1', operations: [] });
  mocks.api.action_undo.mockResolvedValue({ gestureId: 'g-1', operations: [], failures: [] });
  mocks.api.snooze_set.mockResolvedValue({ gestureId: 's-1', operations: [] });
  mocks.api.snooze_clear.mockResolvedValue({ gestureId: 'u-1', operations: [] });
});

describe('P6.3/P6.5 gesture undo stack', () => {
  it('sends one gesture with account-qualified targets and remembers its id', async () => {
    await dispatchGesture(
      [
        { accountId: 'a', threadId: 't1' },
        { accountId: 'b', threadId: 't2' },
      ],
      { kind: 'archive' },
    );

    expect(mocks.api.threads_action).toHaveBeenCalledWith({
      gestureId: null,
      targets: [
        { accountId: 'a', threadId: 't1' },
        { accountId: 'b', threadId: 't2' },
      ],
      action: { kind: 'archive' },
    });
    expect(undoGroups().map((g) => g.gestureId)).toEqual(['g-1']);
  });

  it('undoes the exact gesture a toast belongs to, not the newest one', async () => {
    await dispatchGesture([{ accountId: 'a', threadId: 't1' }], { kind: 'archive' });
    mocks.api.threads_action.mockResolvedValue({ gestureId: 'g-2', operations: [] });
    await dispatchGesture([{ accountId: 'a', threadId: 't2' }], { kind: 'trash' });

    await undoGesture('g-1');

    expect(mocks.api.action_undo).toHaveBeenCalledWith({ gestureId: 'g-1' });
    // The other gesture is still undoable.
    expect(undoGroups().map((g) => g.gestureId)).toEqual(['g-2']);
  });

  it('reports partial per-account undo failures instead of claiming success', async () => {
    mocks.api.action_undo.mockResolvedValue({
      gestureId: 'g-1',
      operations: [],
      failures: [{ accountId: 'b', code: 'offline', message: 'No connection' }],
    });
    await dispatchGesture([{ accountId: 'a', threadId: 't1' }], { kind: 'archive' });

    await undoLast();

    expect(mocks.toast.error).toHaveBeenCalled();
  });

  it('registers a snooze gesture so the list toast undoes that snooze', async () => {
    await snoozeTargets([{ accountId: 'a', threadId: 't1' }], 42, 'Tomorrow 08:00');

    expect(undoGroups().map((g) => g.gestureId)).toEqual(['s-1']);
    expect(mocks.toast).toHaveBeenCalledWith('Snoozed until Tomorrow 08:00', expect.anything());

    // The toast's Undo calls the registered gesture, not whatever is on top.
    registerUndo('s-9', 'later');
    await undoGesture('s-9');
    expect(mocks.api.action_undo).toHaveBeenCalledWith({ gestureId: 's-9' });
  });

  it('unsnooze calls snooze_clear with account-qualified targets', async () => {
    await unsnoozeTargets([{ accountId: 'a', threadId: 't1' }]);

    expect(mocks.api.snooze_clear).toHaveBeenCalledWith({
      gestureId: null,
      targets: [{ accountId: 'a', threadId: 't1' }],
    });
    expect(mocks.toast).toHaveBeenCalledWith('Moved back to Inbox');
  });
});

describe('P6.2 typed error reading', () => {
  it('reads the settled state from an object rejection', () => {
    const err = readSiftError({
      code: 'send_undo_expired',
      message: 'Already sent',
      retryable: false,
      detail: { state: 'done', opId: 3 },
    });
    expect(err.code).toBe('send_undo_expired');
    expect(err.detail?.state).toBe('done');
  });

  it('reads a rejection that arrives as a JSON string', () => {
    const err = readSiftError(
      JSON.stringify({ code: 'acknowledge_duplicate_risk', message: 'May duplicate', retryable: false }),
    );
    expect(err.code).toBe('acknowledge_duplicate_risk');
    expect(err.message).toBe('May duplicate');
  });

  it('keeps a plain message rather than inventing a code', () => {
    const err = readSiftError(new Error('storage_unavailable'));
    expect(err.code).toBe('unknown_error');
    expect(err.message).toBe('storage_unavailable');
  });
});
