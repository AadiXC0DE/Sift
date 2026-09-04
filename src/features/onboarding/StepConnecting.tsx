import React, { useEffect } from 'react';
import { api } from '../../app/ipc/commands';
import { IMAP_ERROR_COPY, useSetup } from '../../stores/setupStore';
import { Button } from '../../ui/Button';

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

/** If the backend goes silent for this long (no progress event, no
 *  resolution), surface an error instead of spinning forever. */
const STALL_MS = 45_000;

type Progress = import('../../app/ipc/types').SetupProgress;

/** One backend sign-in per (email, password), shared across React StrictMode's
 *  double mount and across re-renders. The listener is swapped on each mount
 *  so progress always reaches the live component. */
let inflight: {
  key: string;
  promise: Promise<unknown>;
  listener: (p: Progress) => void;
} | null = null;

function errorCodeOf(p: Progress): string | null {
  if (!p || typeof p !== 'object' || !('error' in p)) return null;
  const e = (p as { error: unknown }).error;
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object' && typeof (e as { code?: unknown }).code === 'string')
    return (e as { code: string }).code;
  return 'imap_transient';
}

function errorCodeFromReject(e: unknown): string {
  const direct = (e as { code?: string })?.code;
  if (typeof direct === 'string') return direct;
  try {
    return JSON.parse(String((e as Error)?.message ?? ''))?.code ?? 'imap_transient';
  } catch {
    return 'imap_transient';
  }
}

export function StepConnecting({ onDone }: { onDone: () => void }) {
  const email = useSetup((s) => s.email);
  const appPassword = useSetup((s) => s.appPassword);
  const progress = useSetup((s) => s.progress);
  const set = useSetup((s) => s.set);
  const attempt = React.useRef(0);
  const stall = React.useRef<ReturnType<typeof setTimeout> | null>(null);

  const start = React.useCallback(
    (fresh: boolean) => {
      const id = ++attempt.current;
      const live = () => id === attempt.current;
      const disarm = () => {
        if (stall.current) clearTimeout(stall.current);
        stall.current = null;
      };
      const arm = () => {
        disarm();
        stall.current = setTimeout(() => {
          if (live()) set({ progress: 'error:imap_transient' });
        }, STALL_MS);
      };
      const onProgress = (p: Progress) => {
        if (!live()) return;
        const code = errorCodeOf(p);
        if (code) {
          disarm();
          set({ progress: `error:${code}` });
        } else if (typeof p === 'string') {
          set({ progress: p });
          arm();
        }
      };
      const key = `${email}\u0000${appPassword}`;
      let req = inflight;
      if (fresh || !req || req.key !== key) {
        const promise = api.accounts_add_app_password(email, appPassword, (p) => inflight?.listener(p));
        req = { key, promise, listener: onProgress };
        inflight = req;
        promise.finally(() => {
          if (inflight === req) inflight = null;
        });
      } else {
        req.listener = onProgress;
      }
      set({ progress: 'connecting' });
      arm();
      req.promise
        .then(() => {
          if (!live()) return;
          disarm();
          set({ progress: 'done' });
          onDone();
        })
        .catch((e) => {
          if (!live()) return;
          disarm();
          set({ progress: `error:${errorCodeFromReject(e)}` });
        });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [email, appPassword],
  );

  useEffect(() => {
    start(false);
    return () => {
      // Invalidate this mount's in-flight sign-in (not a DOM ref).
      attempt.current += 1;
      if (stall.current) clearTimeout(stall.current);
      stall.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const progStr = typeof progress === 'string' ? progress : null;
  const errorCode = progStr?.startsWith('error:') ? progStr.slice(6) : null;
  const copy = errorCode ? (IMAP_ERROR_COPY[errorCode] ?? IMAP_ERROR_COPY.imap_transient) : null;

  return (
    <div style={{ width: 440, display: 'flex', flexDirection: 'column', gap: 12, color: 'var(--fg)' }}>
      <div style={{ fontSize: 20, fontWeight: 650, color: 'var(--fg)' }}>Connecting</div>
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
            <Button onClick={() => set({ step: 'app-password', appPassword: '', progress: null })}>
              Back to app password
            </Button>
          ) : (
            <div style={{ display: 'flex', gap: 8 }}>
              {errorCode === 'imap_web_login_required' && (
                <Button onClick={() => void api.app_open_url('https://mail.google.com')}>Open Gmail</Button>
              )}
              {errorCode === 'imap_all_mail_hidden' && (
                <Button
                  onClick={() => void api.app_open_url('https://mail.google.com/mail/u/0/#settings/labels')}
                >
                  Open Gmail settings
                </Button>
              )}
              <Button variant="primary" onClick={() => start(true)}>
                Try again
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
