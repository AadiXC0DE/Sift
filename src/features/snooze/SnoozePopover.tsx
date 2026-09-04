import React, { useState } from 'react';
import { Popover } from '../../ui/Popover';
import { api } from '../../app/ipc/commands';
import { toast } from 'sonner';

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
}: {
  accountId: string;
  threadIds: string[];
  trigger: React.ReactElement;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const [custom, setCustom] = useState('');
  const go = async (wake: number, label: string) => {
    await api.snooze_set(accountId, threadIds, wake);
    toast(`Snoozed until ${label}`, {
      action: { label: 'Undo', onClick: () => void api.snooze_clear(accountId, threadIds) },
    });
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
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 220 }}>
          {(
            [
              ['Later today 18:00', 'later'],
              ['Tomorrow 08:00', 'tomorrow'],
              ['This weekend', 'weekend'],
              ['Next week', 'nextweek'],
            ] as const
          ).map(([label, k]) => (
            <button
              key={k}
              onClick={() => void go(preset(k), label)}
              style={{
                textAlign: 'left',
                background: 'none',
                border: 'none',
                padding: '8px',
                fontSize: 13,
                cursor: 'pointer',
                borderRadius: 6,
              }}
              className="hoverable"
            >
              {label}
            </button>
          ))}
          <div style={{ display: 'flex', gap: 6, padding: 8 }}>
            <input
              type="datetime-local"
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
        </div>
      }
    />
  );
}
