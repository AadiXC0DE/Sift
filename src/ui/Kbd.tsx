import React from 'react';
export function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <span
      style={{
        fontFamily: 'var(--font-mono)',
        fontSize: 11,
        border: '1px solid var(--border-strong)',
        borderRadius: 4,
        background: 'var(--n2)',
        padding: '1px 5px',
        whiteSpace: 'nowrap',
        color: 'var(--fg-2)',
      }}
    >
      {children}
    </span>
  );
}
