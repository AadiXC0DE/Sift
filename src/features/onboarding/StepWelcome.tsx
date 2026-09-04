import React, { useEffect, useRef } from 'react';
import { api } from '../../app/ipc/commands';
import { useSetup } from '../../stores/setupStore';

const card: React.CSSProperties = {
  width: 440,
  textAlign: 'center',
  display: 'flex',
  flexDirection: 'column',
  gap: 12,
};

export function StepWelcome() {
  const set = useSetup((s) => s.set);
  const oauthAvailable = useSetup((s) => s.oauthAvailable);
  const firstBtn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    firstBtn.current?.focus();
  }, []);
  return (
    <div style={card}>
      <div style={{ fontSize: 28, fontWeight: 700, letterSpacing: '-0.02em' }}>Sift</div>
      <div style={{ fontSize: 14, color: 'var(--fg-2)' }}>Fast, quiet email for Gmail.</div>
      <button
        ref={firstBtn}
        onClick={() => set({ step: 'email' })}
        onKeyDown={(e) => {
          if (e.key === 'Enter') set({ step: 'email' });
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
        Connect your Gmail
      </button>
      {oauthAvailable && (
        <button
          onClick={() => {
            api
              .accounts_add_google()
              .then(() => set({ step: 'connecting', progress: 'syncing' }))
              .catch(() => undefined);
          }}
          style={{
            background: 'none',
            border: 'none',
            color: 'var(--fg-2)',
            fontSize: 13,
            cursor: 'pointer',
          }}
        >
          Sign in with Google instead
        </button>
      )}
      <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>
        Works with Gmail and Google Workspace. Your mail stays on this Mac.
      </div>
    </div>
  );
}
