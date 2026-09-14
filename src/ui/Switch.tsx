import React from 'react';
import { Switch as BaseSwitch } from '@base-ui-components/react/switch';

export function Switch({
  checked,
  onChange,
  label,
  ariaLabel,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
  /** For rows whose visible label sits outside the control (Settings → General). */
  ariaLabel?: string;
}) {
  // The control renders as `<span role="switch">`, and a span is not a
  // labelable element: the wrapping `<label>` below names nothing, so a visible
  // `label` has to be attached to the control by id or the switch is announced
  // with no name at all (P9.5).
  const labelId = React.useId();
  return (
    <label style={{ display: 'inline-flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}>
      <BaseSwitch.Root
        checked={checked}
        onCheckedChange={onChange}
        aria-label={label ? undefined : ariaLabel}
        aria-labelledby={label ? labelId : undefined}
        style={{
          width: 36,
          height: 22,
          borderRadius: 999,
          background: checked ? 'var(--accent)' : 'var(--n4)',
          position: 'relative',
          transition: 'background 140ms ease',
          border: 'none',
        }}
      >
        <BaseSwitch.Thumb
          style={{
            width: 16,
            height: 16,
            borderRadius: '50%',
            background: '#fff',
            position: 'absolute',
            top: 3,
            left: checked ? 17 : 3,
            transition: 'left 140ms var(--ease-out)',
          }}
        />
      </BaseSwitch.Root>
      {label && (
        <span id={labelId} style={{ fontSize: 13 }}>
          {label}
        </span>
      )}
    </label>
  );
}
