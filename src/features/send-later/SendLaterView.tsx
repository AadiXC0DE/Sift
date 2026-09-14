import React, { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { api } from '../../app/ipc/commands';
import type { Draft, OutboxOp } from '../../app/ipc/types';
import { Button } from '../../ui/Button';
import { EmptyState } from '../../ui/EmptyState';
import { Skeleton } from '../../ui/Skeleton';
import { readSiftError } from '../../lib/siftError';
import { utilities } from '../mail-utilities/ipc';
import { SEND_LATER_COPY, formatInZone, localWallTime, timezoneName } from '../mail-utilities/time';
import { useScheduled, type OutboxOpScheduled } from './scheduledStore';
import { ScheduleDialog, type ScheduleChoice } from './ScheduleDialog';

/**
 * The Send Later view (P8.1): one row per queued send that has a deadline.
 *
 * Every action here goes through the outbox: moving a deadline is
 * `send_reschedule`, sending immediately is `send_now`, and both ways of
 * backing out call `send_cancel`, which is the only operation that reopens the
 * draft. The view never claims a message is unsent because a local future was
 * dropped — it re-reads the rows it just changed.
 */
export function SendLaterView({
  accountIds,
  onEditDraft,
}: {
  accountIds: string[];
  onEditDraft: (draft: Draft) => void;
}) {
  const items = useScheduled((s) => s.items);
  const loading = useScheduled((s) => s.loading);
  const loaded = useScheduled((s) => s.loaded);
  const error = useScheduled((s) => s.error);
  const refresh = useScheduled((s) => s.refresh);
  const [editing, setEditing] = useState<OutboxOp | null>(null);
  const [busy, setBusy] = useState<number | null>(null);

  const key = accountIds.join(',');
  useEffect(() => {
    void refresh(key ? key.split(',') : []);
  }, [key, refresh]);

  const reschedule = async (op: OutboxOp, choice: ScheduleChoice) => {
    setEditing(null);
    setBusy(op.opId);
    try {
      await utilities.send_reschedule({
        opId: op.opId,
        notBefore: choice.notBefore,
        scheduledLocalTime: choice.localTime,
        scheduledTimezone: choice.timezone,
      });
      toast(`Scheduled for ${formatInZone(choice.notBefore, choice.timezone)}`);
    } catch (e) {
      // A send past its claim boundary cannot be moved; the honest report is
      // the backend's, and the row is re-read rather than left as it looked.
      toast.error(readSiftError(e).message);
    } finally {
      setBusy(null);
      void refresh(key ? key.split(',') : []);
    }
  };

  const act = async (op: OutboxOp, what: 'send-now' | 'cancel' | 'edit-draft') => {
    setBusy(op.opId);
    try {
      if (what === 'send-now') {
        await utilities.send_now({ opId: op.opId });
        toast('Sending now');
      } else {
        const draft = await api.send_cancel({ opId: op.opId });
        if (what === 'edit-draft') onEditDraft(draft);
        else toast('Scheduled send cancelled — the draft is still in Drafts');
      }
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
        <span style={{ fontWeight: 600, fontSize: 14, flex: 1 }}>Send Later</span>
        <span className="num" style={{ fontSize: 12, color: 'var(--fg-2)' }}>
          {items.length}
        </span>
      </div>
      <div style={{ padding: '8px 12px', fontSize: 11.5, color: 'var(--fg-3)' }}>{SEND_LATER_COPY}</div>
      <div style={{ flex: 1, overflowY: 'auto', padding: '0 12px 16px' }}>
        {loading && !loaded && <Skeleton h={56} />}
        {error && !loaded && (
          <div role="alert" style={{ fontSize: 12.5, color: 'var(--fg-2)', padding: 12 }}>
            Could not read the outbox: {error}
            <div style={{ marginTop: 8 }}>
              <Button size="sm" onClick={() => void refresh(key ? key.split(',') : [])}>
                Retry
              </Button>
            </div>
          </div>
        )}
        {loaded && items.length === 0 && (
          <EmptyState line="Nothing scheduled" sub="Messages you schedule appear here until they send" />
        )}
        {items.map((op) => {
          const sched = op as OutboxOp & OutboxOpScheduled;
          const zone = sched.scheduledTimezone || timezoneName();
          const when = op.scheduledAt ?? 0;
          const pending = op.state === 'pending';
          return (
            <div
              key={op.opId}
              data-testid={`scheduled-${op.opId}`}
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
              <span style={{ fontSize: 13, fontWeight: 550, overflow: 'hidden', textOverflow: 'ellipsis' }}>
                {op.subject || '(No subject)'}
              </span>
              <span style={{ fontSize: 12, color: 'var(--fg-3)' }}>{op.recipientSummary ?? ''}</span>
              <span style={{ fontSize: 12, color: pending ? 'var(--fg-2)' : 'var(--warning)' }}>
                {op.state === 'inflight'
                  ? `Sending now · was scheduled for ${formatInZone(when, zone)}`
                  : `Sends ${formatInZone(when, zone)} · ${zone}`}
              </span>
              <div style={{ display: 'flex', gap: 6, marginTop: 4, flexWrap: 'wrap' }}>
                <Button
                  size="sm"
                  disabled={!pending || busy === op.opId}
                  onClick={() => setEditing(op)}
                  data-testid={`scheduled-edit-time-${op.opId}`}
                >
                  Edit time
                </Button>
                <Button
                  size="sm"
                  disabled={sched.canSendNow === false || busy === op.opId}
                  onClick={() => void act(op, 'send-now')}
                >
                  Send now
                </Button>
                <Button
                  size="sm"
                  disabled={!pending || busy === op.opId}
                  onClick={() => void act(op, 'edit-draft')}
                >
                  Edit draft
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy === op.opId}
                  onClick={() => void act(op, 'cancel')}
                >
                  Cancel send
                </Button>
              </div>
            </div>
          );
        })}
      </div>
      {editing && (
        <ScheduleDialog
          open
          title="Change send time"
          submitLabel="Move send"
          initialLocal={
            (editing as OutboxOp & OutboxOpScheduled).scheduledLocalTime ??
            localWallTime(new Date(editing.scheduledAt ?? Date.now()))
          }
          onClose={() => setEditing(null)}
          onConfirm={(choice) => void reschedule(editing, choice)}
        />
      )}
    </div>
  );
}
