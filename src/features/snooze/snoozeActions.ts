import { toast } from 'sonner';
import { api } from '../../app/ipc/commands';
import { useSettings } from '../../stores/settingsStore';
import { readSiftError } from '../../lib/siftError';
import type { GestureTarget } from '../../app/ipc/types';
import { registerUndo, undoGesture } from '../actions/dispatch';

/**
 * Snooze is local scheduling with an optional matching remote label, not a
 * cross-device scheduler (P6.5). The backend commits the timer, the Inbox
 * removal and the label change in one transaction; this module only owns the
 * gesture handoff, so both entry points (the reader's popover and the list
 * shortcut) register the returned gesture for Undo — the list toast used to
 * call `undoLast` without ever registering, which undid an unrelated gesture.
 */
export interface SnoozeResult {
  gestureId: string;
}

export async function snoozeTargets(
  targets: GestureTarget[],
  wakeAt: number,
  label: string,
): Promise<SnoozeResult | null> {
  if (!targets.length) return null;
  const wakeUnread = useSettings.getState().settings.wakeSnoozedUnread;
  try {
    const { gestureId } = await api.snooze_set({ gestureId: null, targets, wakeAt, wakeUnread });
    registerUndo(gestureId, `Snoozed until ${label}`);
    const dur = useSettings.getState().settings.undoToastDuration * 1000 || 8000;
    toast(`Snoozed until ${label}`, {
      description: wakeUnread ? 'Returns unread when it wakes' : 'Returns read when it wakes',
      action: { label: 'Undo (z)', onClick: () => void undoGesture(gestureId) },
      duration: dur,
    });
    return { gestureId };
  } catch (e) {
    toast.error(readSiftError(e).message);
    return null;
  }
}

/**
 * Unsnooze ("Clear"): removes the timer and the Snoozed label and restores the
 * membership the thread had before it was snoozed — which is Inbox for a thread
 * snoozed out of the inbox.
 */
export async function unsnoozeTargets(targets: GestureTarget[]): Promise<SnoozeResult | null> {
  if (!targets.length) return null;
  try {
    const { gestureId } = await api.snooze_clear({ gestureId: null, targets });
    toast(targets.length > 1 ? `${targets.length} conversations moved back to Inbox` : 'Moved back to Inbox');
    return { gestureId };
  } catch (e) {
    toast.error(readSiftError(e).message);
    return null;
  }
}
