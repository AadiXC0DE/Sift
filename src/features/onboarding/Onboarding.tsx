import React, { useEffect, useState } from 'react';
import { api } from '../../app/ipc/commands';
import { useSync } from '../../stores/syncStore';
import { useAccounts } from '../../stores/accountsStore';

export function Onboarding() {
  const [state, setState] = useState<'idle' | 'waiting' | 'syncing'>('idle');
  const byAccount = useSync((s) => s.byAccount);
  const refresh = useAccounts((s) => s.refresh);

  useEffect(() => {
    if (state === 'syncing') {
      const t = setTimeout(() => refresh(), 1500);
      return () => clearTimeout(t);
    }
  }, [state, refresh]);

  if (state === 'idle') {
    return (
      <div
        style={{
          position: 'fixed',
          inset: 0,
          background: 'var(--bg-app)',
          zIndex: 40,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <div style={{ width: 380, textAlign: 'center', display: 'flex', flexDirection: 'column', gap: 12 }}>
          <div style={{ fontSize: 28, fontWeight: 700, letterSpacing: '-0.02em' }}>Sift</div>
          <div style={{ fontSize: 14, color: 'var(--fg-2)' }}>Fast, quiet email for Gmail.</div>
          <button
            onClick={() => {
              setState('waiting');
              api
                .accounts_add_google()
                .then(() => {
                  setState('syncing');
                  refresh();
                })
                .catch(() => setState('idle'));
            }}
            style={{
              height: 36,
              borderRadius: 8,
              border: 'none',
              background: 'var(--accent)',
              color: '#fff',
              fontSize: 14,
              fontWeight: 600,
              cursor: 'pointer',
            }}
          >
            Connect Google account
          </button>
        </div>
      </div>
    );
  }

  if (state === 'waiting') {
    return (
      <div
        style={{
          position: 'fixed',
          inset: 0,
          background: 'var(--bg-app)',
          zIndex: 40,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <div style={{ textAlign: 'center' }}>
          <div>Waiting for Google…</div>
          <button onClick={() => setState('idle')} style={{ marginTop: 12 }}>
            Cancel
          </button>
        </div>
      </div>
    );
  }

  const prog = Object.values(byAccount)[0];
  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        background: 'var(--bg-app)',
        zIndex: 40,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      <div style={{ width: 380, textAlign: 'center' }}>
        <div>Syncing your inbox…</div>
        {prog && (
          <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>
            {prog.done}/{prog.total || '…'}
          </div>
        )}
        <div style={{ height: 3, background: 'var(--n3)', borderRadius: 2, marginTop: 12 }}>
          <div
            style={{
              width: prog?.total ? `${(prog.done / prog.total) * 100}%` : '20%',
              height: '100%',
              background: 'var(--accent)',
            }}
          />
        </div>
      </div>
    </div>
  );
}
