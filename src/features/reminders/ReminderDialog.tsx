import React, { useEffect, useMemo, useState } from 'react';
import { toast } from 'sonner';
import { Dialog } from '../../ui/Dialog';
import { Button } from '../../ui/Button';
import { utilities, type MessageTarget, type ReminderRow } from '../mail-utilities/ipc';
import {
  formatInZone,
  localWallTime,
  nextOccurrence,
  resolveLocalWallTime,
  timezoneName,
  tomorrowMorning,
} from '../mail-utilities/time';
import { readSiftError } from '../../lib/siftError';

/**
 * Set, move or remove one conversation's reminder (P8.2).
 *
 * The reminder leaves the message where it is: nothing here archives, marks
 * unread or rewrites a mail timestamp. Reschedule and remove are the same
 * dialog because they act on the same single row, and the copy says which of
 * the two states the user is looking at.
 */
export function ReminderDialog({
  target,
  subject,
  existing,
  onClose,
  onSaved,
}: {
  /** Null closes the dialog. */
  target: MessageTarget | null;
  subject?: string;
  existing: ReminderRow | null;
  onClose: () => void;
  onSaved: (rows: ReminderRow[]) => void;
}) {
  const zone = useMemo(timezoneName, []);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!target) return;
    setDraft(
      localWallTime(new Date(existing ? existing.remindAt : nextOccurrence(new Date(), 0, 18).getTime())),
    );
    setError(null);
    setBusy(false);
  }, [target, existing]);

  const resolution = draft ? resolveLocalWallTime(draft) : null;
  const future = !!resolution && resolution.when.getTime() > Date.now();

  const save = async () => {
    if (!target || !resolution || !future) return;
    setBusy(true);
    try {
      const rows = await utilities.reminder_set({
        targets: [target],
        remindAt: resolution.when.getTime(),
      });
      onSaved(rows);
      onClose();
      toast(`Reminder set for ${formatInZone(resolution.when.getTime(), zone)}`);
    } catch (e) {
      setError(readSiftError(e).message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!target) return;
    setBusy(true);
    try {
      onSaved(await utilities.reminder_clear({ targets: [target] }));
      onClose();
      toast('Reminder removed');
    } catch (e) {
      setError(readSiftError(e).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={target != null} onClose={onClose} title={existing ? 'Reminder' : 'Remind me'} width={420}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
        {subject && (
          <div style={{ fontSize: 13, color: 'var(--fg-2)', overflow: 'hidden', textOverflow: 'ellipsis' }}>
            {subject}
          </div>
        )}
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <Button size="sm" onClick={() => setDraft(localWallTime(new Date(Date.now() + 3600_000)))}>
            In 1 hour
          </Button>
          <Button size="sm" onClick={() => setDraft(localWallTime(nextOccurrence(new Date(), 0, 18)))}>
            This evening
          </Button>
          <Button size="sm" onClick={() => setDraft(localWallTime(tomorrowMorning(new Date())))}>
            Tomorrow 8:00
          </Button>
          <Button size="sm" onClick={() => setDraft(localWallTime(nextOccurrence(new Date(), 7, 8)))}>
            Next week
          </Button>
        </div>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13 }}>
          Remind me at
          <input
            type="datetime-local"
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
              setError(null);
            }}
            aria-label="Remind me at"
            data-testid="reminder-input"
            style={{
              height: 32,
              border: '1px solid var(--border-strong)',
              borderRadius: 6,
              padding: '0 10px',
              background: 'var(--bg-raised)',
              color: 'var(--fg)',
            }}
          />
        </label>
        {resolution && (
          <div style={{ fontSize: 12.5, color: 'var(--fg-2)' }} role="status" aria-live="polite">
            {formatInZone(resolution.when.getTime(), zone)} · {zone}
            {!resolution.exact && (
              <div style={{ color: 'var(--warning)', marginTop: 4 }}>
                {draft.replace('T', ' ')} does not exist in {zone} that day; the reminder fires at{' '}
                {resolution.effective.replace('T', ' ')}.
              </div>
            )}
          </div>
        )}
        <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>
          The message stays where it is — a reminder never archives, marks unread or moves mail. Sift must be
          running to notify you; if notifications are off you will still see it as due here.
        </div>
        {error && (
          <div role="alert" style={{ fontSize: 12.5, color: 'var(--danger)' }}>
            {error}
          </div>
        )}
        <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
          {existing ? (
            <Button variant="danger" disabled={busy} onClick={() => void remove()}>
              Remove reminder
            </Button>
          ) : (
            <span />
          )}
          <div style={{ display: 'flex', gap: 8 }}>
            <Button onClick={onClose}>Cancel</Button>
            <Button variant="primary" disabled={busy || !resolution || !future} onClick={() => void save()}>
              {existing ? 'Move reminder' : 'Set reminder'}
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
