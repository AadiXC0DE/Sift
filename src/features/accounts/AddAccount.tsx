import React, { useState } from 'react';
import { api } from '../../app/ipc/commands';

export function AddAccount({ onDone }: { onDone: () => void }) {
  const [phase, setPhase] = useState<'idle' | 'opening' | 'waiting' | 'exchanging'>('idle');
  const copy: Record<string, string> = {
    idle: 'Connect Google account',
    opening: 'Opening browser…',
    waiting: 'Waiting for Google…',
    exchanging: 'Exchanging code…',
  };
  return (
    <div>
      <button
        onClick={() => {
          setPhase('opening');
          setTimeout(() => setPhase('waiting'), 300);
          api
            .accounts_add_google()
            .then(() => {
              setPhase('idle');
              onDone();
            })
            .catch(() => setPhase('idle'));
        }}
        style={{
          height: 32,
          padding: '0 14px',
          borderRadius: 8,
          border: 'none',
          background: 'var(--accent)',
          color: '#fff',
          cursor: 'pointer',
        }}
      >
        {copy[phase]}
      </button>
      {phase === 'waiting' && (
        <button onClick={() => setPhase('idle')} style={{ marginLeft: 8 }}>
          Cancel
        </button>
      )}
    </div>
  );
}
