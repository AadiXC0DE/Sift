import React, { useEffect, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import { useSetup } from '../../stores/setupStore';
import { Button } from '../../ui/Button';
import { groupAppPassword, isValidAppPassword, looksLikeAppPassword } from './appPassword';

const TWO_STEP_URL = 'https://myaccount.google.com/signinoptions/two-step-verification';
const APP_PASSWORDS_URL = 'https://myaccount.google.com/apppasswords';

function Row({ n, done, children }: { n: number; done: boolean; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', gap: 12, alignItems: 'flex-start', textAlign: 'left' }}>
      <div
        aria-label={done ? `step ${n} done` : `step ${n}`}
        style={{
          width: 24,
          height: 24,
          borderRadius: '50%',
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontSize: 13,
          fontWeight: 700,
          background: done ? 'var(--accent)' : 'var(--n3)',
          color: done ? 'var(--fg-on-accent)' : 'var(--fg-2)',
        }}
      >
        {done ? '✓' : n}
      </div>
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', gap: 8 }}>{children}</div>
    </div>
  );
}

export function StepAppPassword() {
  const email = useSetup((s) => s.email);
  const set = useSetup((s) => s.set);
  const [raw, setRaw] = useState('');
  const [touched2Step, setTouched2Step] = useState(false);
  const [touchedAppPw, setTouchedAppPw] = useState(false);
  const [showWhy, setShowWhy] = useState(false);
  const [hint, setHint] = useState<string | null>(null);
  const [undone, setUndone] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Clipboard assist: only on Step C, only while focused, never logged.
  useEffect(() => {
    async function onFocus() {
      try {
        const text = await navigator.clipboard.readText();
        if (!undone && text && looksLikeAppPassword(text) && !raw) {
          setRaw(text.trim());
          setHint('Pasted from your clipboard');
        }
      } catch {
        // Clipboard unavailable (permissions) — user pastes manually.
      }
    }
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [undone, raw]);

  const valid = isValidAppPassword(raw);

  function open(url: string) {
    void api.app_open_url(url);
  }

  return (
    <div style={{ width: 440, display: 'flex', flexDirection: 'column', gap: 16, color: 'var(--fg)' }}>
      <div
        style={{
          alignSelf: 'flex-start',
          fontSize: 12,
          color: 'var(--fg-2)',
          background: 'var(--n3)',
          borderRadius: 999,
          padding: '4px 10px',
        }}
      >
        for {email} ·{' '}
        <button
          onClick={() => set({ step: 'email' })}
          style={{ background: 'none', border: 'none', color: 'var(--accent)', cursor: 'pointer' }}
        >
          Change
        </button>
      </div>
      <div style={{ fontSize: 20, fontWeight: 650, color: 'var(--fg)' }}>Get an app password</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
        <Row n={1} done={touched2Step}>
          <div style={{ fontSize: 14, color: 'var(--fg)' }}>
            Turn on 2-Step Verification (skip if it&apos;s already on).
          </div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            <Button
              onClick={() => {
                setTouched2Step(true);
                open(TWO_STEP_URL);
              }}
            >
              Open 2-Step Verification
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setTouched2Step(true)}>
              Already on? Skip.
            </Button>
          </div>
        </Row>
        <Row n={2} done={touchedAppPw}>
          <div style={{ fontSize: 14, color: 'var(--fg)' }}>
            Create an app password. Name it <strong>Sift</strong> and press Create.
          </div>
          <Button
            onClick={() => {
              setTouchedAppPw(true);
              open(APP_PASSWORDS_URL);
            }}
            style={{ alignSelf: 'flex-start' }}
          >
            Open App passwords
          </Button>
          <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>
            Google shows a 16-letter password once. Copy it.
          </div>
        </Row>
        <Row n={3} done={valid}>
          <div style={{ fontSize: 14, color: 'var(--fg)' }}>Paste the 16-letter password here.</div>
          <input
            ref={inputRef}
            value={groupAppPassword(raw) || raw}
            onChange={(e) => {
              setRaw(e.target.value);
              setHint(null);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && valid) set({ step: 'connecting', appPassword: raw });
              if (e.key === 'Escape') set({ step: 'email' });
            }}
            placeholder="abcd efgh ijkl mnop"
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            aria-label="16-letter app password"
            style={{
              height: 40,
              borderRadius: 8,
              border: '1px solid var(--border-strong)',
              padding: '0 12px',
              fontSize: 15,
              fontFamily: 'var(--font-mono)',
              letterSpacing: '0.08em',
              background: 'var(--bg-raised)',
              color: 'var(--fg)',
            }}
          />
          {hint && (
            <div style={{ fontSize: 12, color: 'var(--fg-2)' }}>
              {hint}{' '}
              <button
                onClick={() => {
                  setRaw('');
                  setHint(null);
                  setUndone(true);
                }}
                style={{ background: 'none', border: 'none', color: 'var(--accent)', cursor: 'pointer' }}
              >
                Undo
              </button>
            </div>
          )}
          <Button
            variant="primary"
            disabled={!valid}
            onClick={() => set({ step: 'connecting', appPassword: raw })}
            style={{ height: 36, fontSize: 14, fontWeight: 600 }}
          >
            Connect
          </Button>
          <button
            onClick={() => setShowWhy((v) => !v)}
            aria-expanded={showWhy}
            style={{
              background: 'none',
              border: 'none',
              color: 'var(--fg-2)',
              fontSize: 13,
              cursor: 'pointer',
              textAlign: 'left',
              padding: 0,
            }}
          >
            Why an app password?
          </button>
          {showWhy && (
            <div style={{ fontSize: 13, color: 'var(--fg-2)' }}>
              Google lets you give one app its own password instead of your real one. It only works for Sift,
              you can revoke it any time on the same page, and Sift keeps it in your Mac&apos;s Keychain.
              Nothing is sent anywhere except Google.
            </div>
          )}
        </Row>
      </div>
    </div>
  );
}
