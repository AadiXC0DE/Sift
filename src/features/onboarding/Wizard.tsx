import React, { useEffect, useState } from 'react';
import { api } from '../../app/ipc/commands';
import { useSetup } from '../../stores/setupStore';
import { StepAppPassword } from './StepAppPassword';
import { StepConnecting } from './StepConnecting';
import { StepEmail } from './StepEmail';
import { StepWelcome } from './StepWelcome';

const ORDER = ['welcome', 'email', 'app-password', 'connecting'] as const;

export function Wizard({ onDone }: { onDone: () => void }) {
  const step = useSetup((s) => s.step);
  const set = useSetup((s) => s.set);
  const [visible, setVisible] = useState(true);

  useEffect(() => {
    api
      .system_info()
      .then((info) => set({ oauthAvailable: info.oauth_available }))
      .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    // 200ms cross-fade (opacity + 8px translate); instant under reduced motion.
    if (window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) return;
    setVisible(false);
    const t = setTimeout(() => setVisible(true), 10);
    return () => clearTimeout(t);
  }, [step]);

  const idx = ORDER.indexOf(step);

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
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          gap: 16,
          opacity: visible ? 1 : 0,
          transform: visible ? 'none' : 'translateY(8px)',
          transition: window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
            ? 'none'
            : 'opacity 200ms var(--ease-out, ease-out), transform 200ms var(--ease-out, ease-out)',
        }}
      >
        <div style={{ display: 'flex', gap: 6 }} aria-label="setup progress">
          {ORDER.slice(0, 3).map((s, i) => (
            <div
              key={s}
              style={{
                width: 6,
                height: 6,
                borderRadius: '50%',
                background: i <= Math.min(idx, 2) ? 'var(--accent)' : 'var(--n3)',
              }}
            />
          ))}
        </div>
        {step === 'welcome' && <StepWelcome />}
        {step === 'email' && <StepEmail />}
        {step === 'app-password' && <StepAppPassword />}
        {step === 'connecting' && <StepConnecting onDone={onDone} />}
      </div>
    </div>
  );
}
