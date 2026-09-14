import React, { useState } from 'react';
import { toast } from 'sonner';
import { Button } from '../../ui/Button';
import { Dialog } from '../../ui/Dialog';
import { utilities } from '../mail-utilities/ipc';
import { readSiftError } from '../../lib/siftError';

/**
 * Empty Trash (P8.5).
 *
 * The count is fetched and shown before anything is queued, because "empty the
 * trash" is the one action here that cannot be undone. Nothing is deleted until
 * the user has seen how many conversations it covers, and the deletion itself
 * is explicit message IDs through the same operation queue as every other
 * action — never an unbounded blind EXPUNGE.
 */
export function EmptyTrashButton({ accountIds }: { accountIds: string[] }) {
  const [count, setCount] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const preview = async () => {
    setBusy(true);
    try {
      const { count: n } = await utilities.trash_empty_preview({ accountIds });
      setCount(n);
    } catch (e) {
      toast.error(readSiftError(e).message);
    } finally {
      setBusy(false);
    }
  };

  const empty = async () => {
    setBusy(true);
    try {
      const res = await utilities.trash_empty({ accountIds });
      const failures = res.failures?.length ?? 0;
      if (failures)
        toast.error(`Trash emptied in ${res.operations.length} operations; ${failures} accounts failed`);
      else toast('Trash emptied');
      setCount(null);
    } catch (e) {
      toast.error(readSiftError(e).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button
        className="sift-chip-btn"
        onClick={() => void preview()}
        disabled={busy || accountIds.length === 0}
        data-testid="empty-trash"
        title="Empty Trash"
      >
        Empty Trash…
      </button>
      <Dialog open={count != null} onClose={() => setCount(null)} title="Empty Trash" width={420}>
        {count === 0 ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
            <div style={{ fontSize: 13 }}>Trash is already empty.</div>
            <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
              <Button onClick={() => setCount(null)}>Close</Button>
            </div>
          </div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
            <div style={{ fontSize: 13 }}>
              Permanently delete {count} {count === 1 ? 'conversation' : 'conversations'} in Trash? This
              cannot be undone.
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
              <Button onClick={() => setCount(null)}>Cancel</Button>
              <Button variant="danger" disabled={busy} onClick={() => void empty()}>
                Delete permanently
              </Button>
            </div>
          </div>
        )}
      </Dialog>
    </>
  );
}
