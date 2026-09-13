import { create } from 'zustand';
import { api } from '../app/ipc/commands';
import { on } from '../app/ipc/events';
import type { Label } from '../app/ipc/types';

/**
 * Display names for label ids, indexed by `(accountId, labelId)` (P3.6).
 *
 * Provider label ids (`Label_...`, `imap:...`) are only unique within an
 * account: two accounts can both own a label named "Client Work" with
 * different ids. Indexing by account keeps a row's chip, its label view and its
 * mutation target on the same account, so a name can never resolve to the
 * other account's label.
 */
export function labelKey(accountId: string, labelId: string): string {
  return `${accountId}\u0000${labelId}`;
}

interface LabelsState {
  /** `${accountId}\0${labelId}` -> display name. */
  names: Record<string, string>;
  /** Index an already-fetched label list for one account. */
  apply: (accountId: string, labels: Label[]) => void;
  /** Fetch and index the labels of accounts that are not loaded yet. */
  ensure: (accountIds: string[]) => void;
}

const loaded = new Set<string>();
const inflight = new Set<string>();
let subscribed = false;

/**
 * A label rename or a new label changes the names shown on rows and in the
 * reader. Invalidate that account only, then re-index it once.
 */
function subscribe(): void {
  if (subscribed) return;
  subscribed = true;
  on<{ account_id?: string }>('store:labels', (payload) => {
    const accountId = payload?.account_id;
    if (!accountId) return;
    loaded.delete(accountId);
    useLabels.getState().ensure([accountId]);
  }).catch(() => {
    // No event bridge (plain browser): names stay as last fetched.
  });
}

export const useLabels = create<LabelsState>((set) => ({
  names: {},
  apply: (accountId, labels) =>
    set((s) => {
      const names = { ...s.names };
      for (const l of labels) names[labelKey(accountId, l.id)] = l.name;
      return { names };
    }),
  ensure: (accountIds) => {
    subscribe();
    for (const id of accountIds) {
      if (!id || loaded.has(id) || inflight.has(id)) continue;
      inflight.add(id);
      void api
        .labels_list(id)
        .then((labels) => {
          loaded.add(id);
          useLabels.getState().apply(id, labels);
        })
        .catch(() => {
          // Offline: the account stays unloaded, so the next ensure retries.
        })
        .finally(() => inflight.delete(id));
    }
  },
}));

/** Test seam: forget what has been fetched. */
export function resetLabelIndex(): void {
  loaded.clear();
  inflight.clear();
  useLabels.setState({ names: {} });
}
