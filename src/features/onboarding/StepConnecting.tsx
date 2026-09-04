import React, { useEffect } from 'react';
import { api } from '../../app/ipc/commands';
import { IMAP_ERROR_COPY, useSetup } from '../../stores/setupStore';

const ROWS = [
  { key: 'connecting', label: 'Reaching Gmail' },
  { key: 'authenticating', label: 'Signing in' },
  { key: 'listing', label: 'Reading your labels' },
  { key: 'syncing', label: 'Starting sync' },
] as const;

type RowKey = (typeof ROWS)[number]['key'];

function rowState(progress: string | null, row: RowKey): 'todo' | 'busy' | 'done' {
  if (!progress || progress === 'connecting') return row === 'connecting' ? 'busy' : 'todo';
  const order: RowKey[] = ['connecting', 'authenticating', 'listing', 'syncing'];
  const cur =
    progress === 'done' ? 4 : progress.startsWith('error') ? order.length : order.indexOf(progress as RowKey);
  const idx = order.indexOf(row);
  if (idx < cur) return 'done';
  if (idx === cur && progress !== 'done') return 'busy';
  return 'todo';
}

export function StepConnecting({ onDone }: { onDone: () => void }) {
  const email = useSetup((s) => s.email);
  const appPassword = useSetup((s) => s.appPassword);
  const progress = useSetup((s) => s.progress);
  const set = useSetup((s) => s.set);
  const started = React.useRef(false);

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    set({ progress: 'connecting' });
    api
      .accounts_add_app_password(email, appPassword, (p) => {
        if (typeof p === 'string') set({ progress: p });
        else if (p && typeof p === 'object' && 'error' in p)
          set({ progress: `error:${(p as { error: string }).error}` });
      })
      .then(() => {
        set({ progress: 'done' });
        onDone();
      })
      .catch((e) => {
        const code =
          (e as { code?: string })?.code ??
          (() => {
            try {
              return JSON.parse(String((e as Error)?.message ?? ''))?.code ?? 'imap_transient';
            } catch {
              return 'imap_transient';
            }
          })();
        set({ progress: `error:${code}` });
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const progStr = typeof progress === 'string' ? progress : null;
  const errorCode = progStr?.startsWith('error:') ? progStr.slice(6) : null;
  const copy = errorCode ? (IMAP_ERROR_COPY[errorCode] ?? IMAP_ERROR_COPY.imap_transient) : null;

  return (
    <div style={{ width: 440, display: 'flex', flexDirection: 'column', gap: 12 }}>
      <div style={{ fontSize: 20, fontWeight: 650 }}>Connecting</div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }} role="status">
        {ROWS.map((r) => {
          const st = errorCode ? (r.key === 'connecting' ? 'todo' : 'todo') : rowState(progStr, r.key);
          const failed =
            !!errorCode &&
            (r.key === 'authenticating' || (errorCode === 'imap_all_mail_hidden' && r.key === 'listing'));
          return (
            <div key={r.key} style={{ display: 'flex', gap: 10, alignItems: 'center' }}>
              <span aria-label={`${r.label}: ${failed ? 'failed' : st}`}>
                {failed ? (
                  <span style={{ color: 'var(--danger)' }}>✕</span>
                ) : st === 'done' ? (
                  <span style={{ color: 'var(--accent)' }}>✓</span>
                ) : st === 'busy' ? (
                  <span className="spin" aria-hidden>
                    ◌
                  </span>
                ) : (
                  <span style={{ color: 'var(--fg-3)' }}>○</span>
                )}
              </span>
              <span style={{ fontSize: 14 }}>{r.label}</span>
            </div>
          );
        })}
      </div>
      {copy && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ fontSize: 13, color: 'var(--danger)' }}>{copy.text}</div>
          {copy.button === 'Back to app password' ? (
            <button
              onClick={() => set({ step: 'app-password', appPassword: '', progress: null })}
              style={{
                height: 34,
                borderRadius: 8,
                border: '1px solid var(--border)',
                background: 'var(--bg-raised)',
                fontSize: 13,
                fontWeight: 600,
                cursor: 'pointer',
              }}
            >
              Back to app password
            </button>
          ) : (
            <div style={{ display: 'flex', gap: 8 }}>
              {errorCode === 'imap_web_login_required' && (
                <button
                  onClick={() => void api.app_open_url('https://mail.google.com')}
                  style={{
                    height: 34,
                    borderRadius: 8,
                    border: '1px solid var(--border)',
                    background: 'var(--bg-raised)',
                    padding: '0 12px',
                    fontSize: 13,
                    fontWeight: 600,
                    cursor: 'pointer',
                  }}
                >
                  Open Gmail
                </button>
              )}
              {errorCode === 'imap_all_mail_hidden' && (
                <button
                  onClick={() => void api.app_open_url('https://mail.google.com/mail/u/0/#settings/labels')}
                  style={{
                    height: 34,
                    borderRadius: 8,
                    border: '1px solid var(--border)',
                    background: 'var(--bg-raised)',
                    padding: '0 12px',
                    fontSize: 13,
                    fontWeight: 600,
                    cursor: 'pointer',
                  }}
                >
                  Open Gmail settings
                </button>
              )}
              <button
                onClick={() => {
                  set({ progress: null });
                  started.current = false;
                  set({ progress: 'connecting' });
                }}
                style={{
                  height: 34,
                  borderRadius: 8,
                  border: 'none',
                  background: 'var(--accent)',
                  color: '#fff',
                  padding: '0 12px',
                  fontSize: 13,
                  fontWeight: 600,
                  cursor: 'pointer',
                }}
              >
                Try again
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
