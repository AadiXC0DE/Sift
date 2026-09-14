import { create } from 'zustand';
import { api } from '../../app/ipc/commands';
import type { OutboxOp } from '../../app/ipc/types';

/**
 * Scheduled sends, as the outbox already holds them (P8.1).
 *
 * There is no second schedule store: a queued send with a `scheduled_at` *is*
 * the schedule, so the Send Later view, the sidebar row and the composer all
 * read the one durable row the outbox will claim at its deadline. A separate
 * list would be able to disagree with what actually sends.
 */
interface ScheduledState {
  items: OutboxOp[];
  loading: boolean;
  /** True once a refresh has completed, so an empty list is not "not asked yet". */
  loaded: boolean;
  error: string | null;
  refresh: (accountIds: string[]) => Promise<void>;
}

/**
 * `scheduledLocalTime`/`scheduledTimezone`/`canSendNow` are part of the same
 * contract change as `scheduledAt`; reading them as optional keeps this module
 * honest when a queued row predates the fields instead of inventing a zone.
 */
export interface OutboxOpScheduled {
  scheduledLocalTime?: string | null;
  scheduledTimezone?: string | null;
  canSendNow?: boolean;
}

export const useScheduled = create<ScheduledState>((set) => ({
  items: [],
  loading: false,
  loaded: false,
  error: null,
  refresh: async (accountIds) => {
    if (!accountIds.length) {
      set({ items: [], loading: false, loaded: true, error: null });
      return;
    }
    set({ loading: true });
    try {
      const page = await api.outbox_list({
        accountIds,
        states: ['pending', 'inflight'],
        limit: 50,
      });
      set({
        items: page.operations.filter((op) => op.scheduledAt != null),
        loading: false,
        loaded: true,
        error: null,
      });
    } catch (e) {
      // A failed list must not read as "nothing scheduled": the row count is
      // what decides whether the feature is navigable at all (P8.1).
      const message = e instanceof Error ? e.message : String(e);
      set({ items: [], loading: false, loaded: false, error: message });
    }
  },
}));
