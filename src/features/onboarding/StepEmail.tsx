import React, { useEffect, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import { useSetup } from '../../stores/setupStore';
import { Button } from '../../ui/Button';

export function StepEmail() {
  const email = useSetup((s) => s.email);
  const set = useSetup((s) => s.set);
  const [value, setValue] = useState(email);
  const [checking, setChecking] = useState(false);
  const [showAnyway, setShowAnyway] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  async function cont(next?: string) {
    const addr = (next ?? value).trim();
    if (!addr.includes('@')) return;
    setChecking(true);
    try {
      const hosted = await api.accounts_probe_email(addr).catch(() => null);
      set({ email: addr, googleHosted: hosted });
      if (hosted === false) {
        setShowAnyway(true);
        setChecking(false);
        return;
      }
      set({ step: 'app-password' });
    } finally {
      setChecking(false);
    }
  }

  return (
    <div style={{ width: 440, display: 'flex', flexDirection: 'column', gap: 12, color: 'var(--fg)' }}>
      <div style={{ fontSize: 20, fontWeight: 650, color: 'var(--fg)' }}>Your address</div>
      <input
        ref={inputRef}
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          setShowAnyway(false);
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void cont();
          if (e.key === 'Escape') set({ step: 'welcome' });
        }}
        placeholder="you@gmail.com"
        inputMode="email"
        autoComplete="email"
        style={{
          height: 36,
          borderRadius: 8,
          border: '1px solid var(--border-strong)',
          padding: '0 12px',
          fontSize: 14,
          background: 'var(--bg-raised)',
          color: 'var(--fg)',
        }}
      />
      {showAnyway && (
        <div style={{ fontSize: 13, color: 'var(--fg-2)' }}>
          This doesn&apos;t look like a Google-hosted address. Sift works with Gmail and Google Workspace for
          now.{' '}
          <button
            onClick={() => {
              set({ email: value.trim(), step: 'app-password' });
            }}
            style={{ background: 'none', border: 'none', color: 'var(--accent)', cursor: 'pointer' }}
          >
            Try anyway
          </button>
        </div>
      )}
      <Button
        variant="primary"
        onClick={() => void cont()}
        disabled={!value.includes('@') || checking}
        style={{ height: 36, fontSize: 14, fontWeight: 600 }}
      >
        Continue
      </Button>
    </div>
  );
}
