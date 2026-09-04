import React from 'react';
export function Chip({ label, color = 'var(--accent)' }: { label: string; color?: string }) {
  return (
    <span
      style={{
        height: 20,
        fontSize: 11.5,
        borderRadius: 5,
        padding: '0 7px',
        display: 'inline-flex',
        alignItems: 'center',
        background: `color-mix(in oklab, ${color} 14%, transparent)`,
        color,
        whiteSpace: 'nowrap',
        overflow: 'hidden',
        maxWidth: 140,
      }}
    >
      {label}
    </span>
  );
}
