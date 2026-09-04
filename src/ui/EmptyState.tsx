import React from 'react';
export function EmptyState({ icon, line, sub }: { icon?: React.ReactNode; line: string; sub?: string }) {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        height: '100%',
        gap: 8,
        color: 'var(--fg-3)',
      }}
    >
      {icon}
      <div style={{ fontSize: 14, color: 'var(--fg-2)' }}>{line}</div>
      {sub && <div style={{ fontSize: 12 }}>{sub}</div>}
    </div>
  );
}
