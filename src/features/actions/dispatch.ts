import { toast } from 'sonner';
import { api } from '../../app/ipc/commands';
import type { ActionKind, GestureTarget, ThreadAction } from '../../app/ipc/types';
import { useSettings } from '../../stores/settingsStore';
import { readSiftError } from '../../lib/siftError';

export interface UndoEntry {
  gestureId: string;
  at: number;
  label: string;
}

const undoStack: UndoEntry[] = [];

/** One undo window for every gesture, matching the toast lifetime. */
const UNDO_WINDOW_MS = 60_000;

export function undoGroups(): UndoEntry[] {
  return undoStack;
}

/**
 * Register a gesture that was not created by `dispatchGesture` — the snooze
 * service returns the same `{gestureId, operations}` shape, and its toast's
 * Undo must act on that exact gesture rather than on whatever happens to be on
 * top of the stack (P6.5).
 */
export function registerUndo(gestureId: string, label: string): void {
  undoStack.push({ gestureId, at: Date.now(), label });
  pruneUndo();
}

function pruneUndo(): void {
  while (undoStack.length && Date.now() - undoStack[0].at > UNDO_WINDOW_MS) undoStack.shift();
}

function dropGesture(gestureId: string): void {
  const i = undoStack.findIndex((e) => e.gestureId === gestureId);
  if (i >= 0) undoStack.splice(i, 1);
}

/**
 * Undo one exact gesture. A cross-account selection is one gesture with targets
 * in several accounts (P6.3), so per-account failures are reported as failures
 * of that same gesture rather than as separate undos that may not exist.
 */
export async function undoGesture(gestureId: string): Promise<void> {
  dropGesture(gestureId);
  try {
    const { failures } = await api.action_undo({ gestureId });
    if (failures?.length) {
      toast.error(
        failures.length === 1
          ? `Couldn't undo in one account: ${failures[0].message}`
          : `Couldn't undo ${failures.length} accounts`,
      );
    }
  } catch (e) {
    toast.error(readSiftError(e).message);
  }
}

export function actionLabel(action: ActionKind, targetCount: number): string {
  const n = targetCount;
  const conversations = n > 1 ? `${n} conversations` : 'Conversation';
  switch (action.kind) {
    case 'archive':
      return n > 1 ? `${n} conversations archived` : 'Archived';
    case 'trash':
      return n > 1 ? `${n} conversations moved to Trash` : 'Moved to Trash';
    case 'spam':
      return 'Marked as spam';
    case 'star':
      return action.on ? 'Starred' : 'Unstarred';
    case 'read':
      return action.on ? 'Marked as read' : 'Marked as unread';
    default:
      return `${conversations} updated`;
  }
}

/**
 * Run one user gesture over account-qualified targets. The backend commits one
 * transaction per account and returns one gesture id (P6.3): a mixed-account
 * selection is never split into unrelated undo groups here.
 */
export async function dispatchGesture(
  targets: GestureTarget[],
  action: ActionKind,
  opts?: { silent?: boolean },
): Promise<string | null> {
  if (!targets.length) return null;
  try {
    const { gestureId } = await api.threads_action({ gestureId: null, targets, action });
    const label = actionLabel(action, targets.length);
    registerUndo(gestureId, label);
    if (!opts?.silent) {
      const dur = useSettings.getState().settings.undoToastDuration * 1000 || 8000;
      toast(label, {
        action: { label: 'Undo (z)', onClick: () => void undoGesture(gestureId) },
        duration: dur,
      });
    }
    return gestureId;
  } catch (e) {
    const err = readSiftError(e);
    if (err.code === 'not_in_trash') {
      toast.error('Only Trash or Spam can be deleted forever');
    } else {
      toast.error(`${err.message}. Will retry`, {
        action: { label: 'Retry', onClick: () => void dispatchGesture(targets, action, opts) },
      });
    }
    return null;
  }
}

/**
 * One gesture over one account's thread ids. This is the shape every in-list and
 * in-reader call site already uses; it is a target translation, not a second
 * dispatch path.
 */
export function dispatchAction(action: ThreadAction, opts?: { silent?: boolean }): Promise<string | null> {
  return dispatchGesture(
    action.threadIds.map((threadId) => ({ accountId: action.accountId, threadId })),
    action.action,
    opts,
  );
}

/** Undo the most recent gesture from the keyboard binding or the palette. */
export async function undoLast(): Promise<void> {
  pruneUndo();
  const top = undoStack.pop();
  if (!top) return;
  await undoGesture(top.gestureId);
}
