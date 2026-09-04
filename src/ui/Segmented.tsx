import React from 'react';

export function Segmented<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
}) {
  return (
    <div
      style={{
        display: 'inline-flex',
        background: 'var(--n2)',
        borderRadius: 'var(--r-md)',
        padding: 2,
        gap: 2,
        position: 'relative',
      }}
    >
      {options.map((o) => (
        <button
          key={o.value}
          onClick={() => onChange(o.value)}
          style={{
            padding: '5px 12px',
            fontSize: 13,
            borderRadius: 6,
            border: 'none',
            cursor: 'pointer',
            background: o.value === value ? 'var(--n0)' : 'transparent',
            color: 'var(--fg)',
            boxShadow: o.value === value ? '0 1px 3px rgb(0 0 0 / .12)' : 'none',
            transition: 'background 160ms var(--ease-out)',
          }}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
