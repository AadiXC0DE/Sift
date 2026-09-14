import { create } from 'zustand';
import { utilities, type ReminderRow } from '../mail-utilities/ipc';
/**
 * Local message reminders (P8.2).
 *
 * A reminder is a local row keyed by `(account, thread)`; it never touches a
 * provider label or a mail timestamp. The store therefore only mirrors what
 * the backend holds — it is never the place a reminder "exists".
 */
interface RemindersState {
  rows: ReminderRow[];
  loaded: boolean;
  error: string | null;
  refresh: (accountIds: string[]) => Promise<void>;
}

export const useReminders = create<RemindersState>((set) => ({
  rows: [],
  loaded: false,
  error: null,
  refresh: async (accountIds) => {
    if (!accountIds.length) {
      set({ rows: [], loaded: true, error: null });
      return;
    }
    try {
      const rows = await utilities.reminders_list({ accountIds, includeCompleted: false });
      set({ rows, loaded: true, error: null });
    } catch (e) {
      // A failed read must not look like "no reminders": the sidebar row is
      // what makes the view reachable, and hiding it on an error would hide
      // reminders the user still has.
      set({ loaded: false, error: e instanceof Error ? e.message : String(e) });
    }
  },
}));
