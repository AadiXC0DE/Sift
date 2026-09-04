import React from 'react';
import { Switch as BaseSwitch } from '@base-ui-components/react/switch';

export function Switch({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
}) {
  return (
    <label style={{ display: 'inline-flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}>
      <BaseSwitch.Root
        checked={checked}
        onCheckedChange={onChange}
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
      {label && <span style={{ fontSize: 13 }}>{label}</span>}
    </label>
  );
}
