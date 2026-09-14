import React, { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { Button } from '../../ui/Button';
import { EmptyState } from '../../ui/EmptyState';
import { useReminders } from './remindersStore';
import { ReminderDialog } from './ReminderDialog';
import { utilities, type ReminderRow } from '../mail-utilities/ipc';
import { formatInZone, timezoneName } from '../mail-utilities/time';
import { readSiftError } from '../../lib/siftError';

/**
 * A due reminder whose notification never went out (P8.2).
 *
 * The backend marks delivery before it notifies, so `due` means exactly one
 * thing: the deadline passed and no delivery was confirmed — normally because
 * notifications are denied or Sift was not running. Saying so is the point:
 * the reminder is still visible instead of silently failing.
 */
const STATE_COPY: Record<ReminderRow['state'], string> = {
  scheduled: 'Scheduled',
  due: 'Due — no notification was delivered',
  delivered: 'Notified',
  completed: 'Done',
};

export function RemindersView({
  accountIds,
  onOpenThread,
}: {
  accountIds: string[];
  onOpenThread: (ref: { accountId: string; threadId: string }) => void;
}) {
  const rows = useReminders((s) => s.rows);
  const loaded = useReminders((s) => s.loaded);
  const error = useReminders((s) => s.error);
  const refresh = useReminders((s) => s.refresh);
  const [editing, setEditing] = useState<ReminderRow | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const zone = timezoneName();

  const key = accountIds.join(',');
  useEffect(() => {
    void refresh(key ? key.split(',') : []);
  }, [key, refresh]);

  const remove = async (row: ReminderRow) => {
    setBusy(`${row.accountId}:${row.threadId}`);
    try {
      await utilities.reminder_clear({
        targets: [{ accountId: row.accountId, threadId: row.threadId }],
      });
      toast('Reminder removed');
    } catch (e) {
      toast.error(readSiftError(e).message);
    } finally {
      setBusy(null);
      void refresh(key ? key.split(',') : []);
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}>
      <div
        style={{
          height: 'var(--toolbar-h)',
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          padding: '0 12px',
          borderBottom: '1px solid var(--border)',
          flexShrink: 0,
        }}
      >
        <span style={{ fontWeight: 600, fontSize: 14, flex: 1 }}>Reminders</span>
        <span className="num" style={{ fontSize: 12, color: 'var(--fg-2)' }}>
          {rows.length}
        </span>
      </div>
      <div style={{ flex: 1, overflowY: 'auto', padding: '8px 12px 16px' }}>
        {!loaded && error && (
          <div role="alert" style={{ fontSize: 12.5, color: 'var(--fg-2)' }}>
            Could not read reminders: {error}
            <div style={{ marginTop: 8 }}>
              <Button size="sm" onClick={() => void refresh(key ? key.split(',') : [])}>
                Retry
              </Button>
            </div>
          </div>
        )}
        {loaded && rows.length === 0 && (
          <EmptyState line="No reminders" sub="Remind me… from a message leaves it in place" />
        )}
        {rows.map((row) => {
          const id = `${row.accountId}:${row.threadId}`;
          return (
            <div
              key={id}
              data-testid={`reminder-${id}`}
              style={{
                border: '1px solid var(--border)',
                borderRadius: 'var(--r-lg)',
                padding: 10,
                marginBottom: 8,
                background: 'var(--n0)',
                display: 'flex',
                flexDirection: 'column',
                gap: 4,
              }}
            >
              <div style={{ display: 'flex', gap: 8, alignItems: 'baseline' }}>
                <span
                  style={{
                    flex: 1,
                    minWidth: 0,
                    fontSize: 13,
                    fontWeight: row.unread ? 600 : 550,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {row.subject || '(No subject)'}
                </span>
                <span style={{ fontSize: 12, color: 'var(--fg-3)' }}>{row.fromName ?? ''}</span>
              </div>
              <span
                style={{
                  fontSize: 12,
                  color: row.state === 'due' ? 'var(--warning)' : 'var(--fg-2)',
                }}
              >
                {formatInZone(row.remindAt, zone)} · {STATE_COPY[row.state]}
              </span>
              <div style={{ display: 'flex', gap: 6, marginTop: 4 }}>
                <Button size="sm" onClick={() => onOpenThread(row)}>
                  Open
                </Button>
                <Button size="sm" disabled={busy === id} onClick={() => setEditing(row)}>
                  Reschedule
                </Button>
                <Button size="sm" variant="ghost" disabled={busy === id} onClick={() => void remove(row)}>
                  Remove
                </Button>
              </div>
            </div>
          );
        })}
      </div>
      <ReminderDialog
        target={editing ? { accountId: editing.accountId, threadId: editing.threadId } : null}
        subject={editing?.subject ?? undefined}
        existing={editing}
        onClose={() => setEditing(null)}
        onSaved={() => void refresh(key ? key.split(',') : [])}
      />
    </div>
  );
}
