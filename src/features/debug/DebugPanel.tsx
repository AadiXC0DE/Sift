import React, { useEffect, useState } from 'react';
import { api } from '../../app/ipc/commands';
import type { SyncStatus } from '../../app/ipc/types';

export function DebugPanel() {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<SyncStatus[]>([]);
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.metaKey && e.shiftKey && e.key.toLowerCase() === 'd') setOpen((v) => !v);
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, []);
  useEffect(() => {
    if (open)
      api
        .sync_status()
        .then(setStatus)
        .catch(() => {});
  }, [open]);
  if (!open) return null;
  if (!import.meta.env.DEV) return null;
  return (
    <div
      style={{
        position: 'fixed',
        right: 12,
        bottom: 12,
        width: 320,
        background: 'var(--bg-elevated)',
        border: '1px solid var(--border)',
        borderRadius: 8,
        padding: 12,
        zIndex: 80,
        fontSize: 12,
      }}
    >
      <div style={{ fontWeight: 600, marginBottom: 8 }}>Debug (⌘⇧D)</div>
      {status.map((s) => (
        <div key={s.account_id}>
          {s.account_id.slice(0, 6)} · {s.phase} {s.done}/{s.total}
        </div>
      ))}
      <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
        <button onClick={() => void api.sync_now()}>Sync now</button>
        <button
          onClick={() => {
            document.querySelector('meta[http-equiv]');
          }}
        >
          CSP check
        </button>
      </div>
    </div>
  );
}
