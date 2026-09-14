import React from 'react';
import { Menu } from '../../ui/Menu';
import { timezoneName, tomorrowMorning } from '../mail-utilities/time';

/**
 * The Send split control (P8.1).
 *
 * "Send now" is the plain send the primary button already performs; the two
 * schedule entries queue the same immutable revision with a future deadline.
 * The zone is named in the menu header because the exact instant a schedule
 * means depends on it, and the user has to see that before anything is queued.
 */
export function SendLaterMenu({
  disabled,
  onSendNow,
  onTomorrow,
  onChoose,
}: {
  disabled: boolean;
  onSendNow: () => void;
  onTomorrow: (at: Date) => void;
  onChoose: () => void;
}) {
  const zone = timezoneName();
  return (
    <Menu
      label={`Scheduled sends use ${zone}`}
      trigger={
        <button
          type="button"
          aria-label="Send later options"
          title="Send later (⌘⇧L)"
          data-testid="send-later-trigger"
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            height: 32,
            width: 32,
            padding: 0,
            marginLeft: -1,
            border: '1px solid transparent',
            borderTopRightRadius: 'var(--r-md)',
            borderBottomRightRadius: 'var(--r-md)',
            borderTopLeftRadius: 0,
            borderBottomLeftRadius: 0,
            background: 'var(--accent)',
            color: 'var(--fg-on-accent)',
            fontSize: 12,
            opacity: disabled ? 0.5 : 1,
            cursor: disabled ? 'default' : 'pointer',
          }}
        >
          ▾
        </button>
      }
      items={[
        { label: 'Send now', hint: '⌘↵', action: () => !disabled && onSendNow() },
        {
          label: 'Tomorrow 8:00',
          action: () => !disabled && onTomorrow(tomorrowMorning(new Date())),
        },
        { label: 'Choose date and time…', action: () => !disabled && onChoose() },
      ]}
    />
  );
}
