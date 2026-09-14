import React, { useState } from 'react';
import { Popover } from '../../ui/Popover';
import { useSettings } from '../../stores/settingsStore';
import { snoozeTargets, unsnoozeTargets } from './snoozeActions';

function preset(name: string): number {
  const d = new Date();
  if (name === 'later') {
    d.setHours(18, 0, 0, 0);
    if (d.getTime() < Date.now()) d.setDate(d.getDate() + 1);
    return d.getTime();
  }
  if (name === 'tomorrow') {
    d.setDate(d.getDate() + 1);
    d.setHours(8, 0, 0, 0);
    return d.getTime();
  }
  if (name === 'weekend') {
    const day = d.getDay();
    const add = (6 - day + 7) % 7 || 7;
    d.setDate(d.getDate() + add);
    d.setHours(8, 0, 0, 0);
    return d.getTime();
  }
  d.setDate(d.getDate() + 7);
  d.setHours(8, 0, 0, 0);
  return d.getTime();
}

export function SnoozeButton({
  accountId,
  threadIds,
  trigger,
  open,
  onOpenChange,
  snoozed = false,
}: {
  accountId: string;
  threadIds: string[];
  trigger: React.ReactElement;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** The target is already snoozed, so Unsnooze is offered alongside the presets. */
  snoozed?: boolean;
}) {
  const [custom, setCustom] = useState('');
  const wakeUnread = useSettings((s) => s.settings.wakeSnoozedUnread);
  const targets = threadIds.map((threadId) => ({ accountId, threadId }));

  const go = async (wake: number, label: string) => {
    await snoozeTargets(targets, wake, label);
    onOpenChange?.(false);
  };

  const item: React.CSSProperties = {
    textAlign: 'left',
    background: 'none',
    border: 'none',
    padding: '8px',
    fontSize: 13,
    cursor: 'pointer',
    borderRadius: 6,
  };

  return (
    <Popover
      trigger={trigger}
      open={open}
      onOpenChange={(o) => {
        onOpenChange?.(o);
        if (!o) setCustom('');
      }}
      children={
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 236 }}>
          {(
            [
              ['Later today 18:00', 'later'],
              ['Tomorrow 08:00', 'tomorrow'],
              ['This weekend', 'weekend'],
              ['Next week', 'nextweek'],
            ] as const
          ).map(([label, k]) => (
            <button key={k} onClick={() => void go(preset(k), label)} style={item} className="hoverable">
              {label}
            </button>
          ))}
          <div style={{ display: 'flex', gap: 6, padding: 8 }}>
            <input
              type="datetime-local"
              aria-label="Snooze until"
              value={custom}
              onChange={(e) => setCustom(e.target.value)}
              style={{ flex: 1 }}
            />
            <button
              onClick={() => custom && void go(new Date(custom).getTime(), custom)}
              style={{ fontSize: 12 }}
            >
              Set
            </button>
          </div>
          {snoozed && (
            <button
              onClick={() => {
                void unsnoozeTargets(targets);
                onOpenChange?.(false);
              }}
              style={{ ...item, borderTop: '1px solid var(--border)', borderRadius: 0 }}
              className="hoverable"
            >
              Unsnooze
            </button>
          )}
          {/*
            Snooze is local scheduling (P6.5): the timer lives on this device and
            only fires while Sift is running with the account reachable, so the
            popover says exactly that instead of implying a server-side alarm.
          */}
          <div
            style={{
              borderTop: '1px solid var(--border)',
              padding: '8px 8px 4px',
              fontSize: 11.5,
              lineHeight: 1.45,
              color: 'var(--fg-3)',
            }}
          >
            Sift must be running and online to wake this.
            <br />
            {wakeUnread ? 'Wakes unread.' : 'Wakes read.'}{' '}
            <span style={{ color: 'var(--fg-3)' }}>(Settings → General)</span>
          </div>
        </div>
      }
    />
  );
}
