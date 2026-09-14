import React from 'react';
import { COLOR_MIX_SUPPORTED } from '../lib/css';

// P9.5 — the tint below is color-mix(); see lib/css.ts for why this is a
// runtime check rather than a CSS fallback. The neutral surface keeps the chip
// readable on both themes when the engine cannot mix.
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
        background: COLOR_MIX_SUPPORTED ? `color-mix(in oklab, ${color} 14%, transparent)` : 'var(--n2)',
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
