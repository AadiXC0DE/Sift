import { toast } from 'sonner';
import { api } from '../../app/ipc/commands';
import type { ThreadAction } from '../../app/ipc/types';
import { useSettings } from '../../stores/settingsStore';

const undoStack: { group: string; at: number; label: string }[] = [];

export function undoGroups(): typeof undoStack {
  return undoStack;
}

export async function dispatchAction(
  action: ThreadAction,
  opts?: { silent?: boolean },
): Promise<string | null> {
  try {
    const { undo_group } = await api.threads_action(action);
    const label = actionLabel(action);
    undoStack.push({ group: undo_group, at: Date.now(), label });
    // prune >60s
    while (undoStack.length && Date.now() - undoStack[0].at > 60_000) undoStack.shift();
    if (!opts?.silent) {
      const dur = useSettings.getState().settings.undoToastDuration * 1000 || 8000;
      toast(label, {
        action: {
          label: 'Undo (z)',
          onClick: () => void api.action_undo(undo_group),
        },
        duration: dur,
      });
      // Undo via the global engine binding (z, 60s window in undoStack) and the toast button.
    }
    return undo_group;
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    if (msg.includes('not_in_trash')) {
      toast.error('Only Trash or Spam can be deleted forever');
    } else {
      toast.error('Action failed. Will retry', {
        action: { label: 'Retry', onClick: () => void dispatchAction(action, opts) },
      });
    }
    return null;
  }
}

function actionLabel(a: ThreadAction): string {
  const n = a.threadIds.length;
  const s = n > 1 ? `${n} conversations` : 'Conversation';
  const k = (a.action as { kind: string }).kind;
  switch (k) {
    case 'archive':
      return n > 1 ? `${n} conversations archived` : 'Archived';
    case 'trash':
      return n > 1 ? `${n} conversations moved to Trash` : 'Moved to Trash';
    case 'spam':
      return 'Marked as spam';
    case 'star':
      return 'Starred';
    case 'read':
      return (a.action as { on: boolean }).on ? 'Marked as read' : 'Marked as unread';
    default:
      return `${s} updated`;
  }
}

export async function undoLast(): Promise<void> {
  const top = undoStack[undoStack.length - 1];
  if (!top || Date.now() - top.at > 60_000) return;
  undoStack.pop();
  await api.action_undo(top.group);
}
