import React, { useMemo } from 'react';
import { useSync } from '../../stores/syncStore';
import { useAccounts } from '../../stores/accountsStore';

export interface SyncProgress {
  active: boolean;
  phase: 'listing' | 'metadata' | 'full' | '';
  done: number;
  total: number;
  failed: string | null;
}

/** Aggregate initial-sync progress for the given accounts. Drives both the
 *  first-run panel and the inline bar shown once rows start arriving. */
export function useSyncProgress(accountIds: string[]): SyncProgress {
  const byAccount = useSync((s) => s.byAccount);
  const accounts = useAccounts((s) => s.accounts);
  const key = accountIds.join(',');
  return useMemo(() => {
    let active = false;
    let failed: string | null = null;
    let phase: SyncProgress['phase'] = '';
    let done = 0;
    let total = 0;
    for (const id of accountIds) {
      const acc = accounts.find((a) => a.id === id);
      const st = byAccount[id];
      if (st && (st.phase === 'listing' || st.phase === 'metadata' || st.phase === 'full')) {
        active = true;
        phase = st.phase;
        if (st.phase === 'metadata') {
          done = Math.max(done, st.done);
          total = Math.max(total, st.total);
        } else if (st.total) {
          total = Math.max(total, st.total);
        }
      } else if (acc && (acc.sync_state === 'new' || acc.sync_state === 'full')) {
        // Account added but the first progress event has not arrived yet.
        active = true;
        phase = phase || 'listing';
      }
      if (acc?.sync_state === 'error') failed = st?.last_error ?? 'Sync paused';
    }
    return { active, phase, done, total, failed };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, byAccount, accounts]);
}

function Bar({ done, total }: { done: number; total: number }) {
  const pct = total > 0 ? Math.min(100, Math.max(3, Math.round((done / total) * 100))) : 0;
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={total || undefined}
      aria-valuenow={total ? done : undefined}
      style={{ height: 4, background: 'var(--n3)', borderRadius: 999, overflow: 'hidden' }}
    >
      <div
        className={total ? undefined : 'sync-indeterminate'}
        style={{
          width: total ? `${pct}%` : '35%',
          height: '100%',
          background: 'var(--accent)',
          borderRadius: 999,
          transition: 'width 240ms var(--ease-out)',
        }}
      />
    </div>
  );
}

/** Full-pane first-run state: shown while the mailbox is downloading and the
 *  list is still empty, so users get clear feedback instead of an empty inbox. */
export function SyncPanel({ progress, onRetry }: { progress: SyncProgress; onRetry?: () => void }) {
  if (progress.failed && !progress.active) {
    return (
      <div style={center}>
        <div style={card}>
          <div style={title}>Sync paused</div>
          <div style={sub}>
            Sift couldn&apos;t finish downloading your mail. Check your connection and try again.
          </div>
          {onRetry && (
            <button onClick={onRetry} style={retryBtn}>
              Try again
            </button>
          )}
        </div>
      </div>
    );
  }
  const hasTotal = progress.total > 0;
  return (
    <div style={center} role="status" aria-live="polite">
      <div style={card}>
        <div style={title}>Getting your mail</div>
        <div style={sub}>
          Sift is copying your mailbox to this Mac so it opens instantly from now on. The list fills in as
          messages arrive.
        </div>
        <Bar done={progress.done} total={progress.total} />
        <div style={count}>
          {hasTotal
            ? `${progress.done.toLocaleString()} of ${progress.total.toLocaleString()} conversations`
            : 'Listing your conversations…'}
        </div>
      </div>
    </div>
  );
}

/** Slim progress bar shown above the list once the first rows have arrived. */
export function SyncInlineBar({ progress }: { progress: SyncProgress }) {
  if (!progress.active) return null;
  const hasTotal = progress.total > 0;
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        padding: '6px 12px',
        borderBottom: '1px solid var(--border)',
        flexShrink: 0,
        background: 'color-mix(in oklab, var(--accent) 5%, transparent)',
      }}
    >
      <span style={{ fontSize: 12, color: 'var(--fg-2)', whiteSpace: 'nowrap' }}>Downloading mail</span>
      <span style={{ flex: 1, minWidth: 40 }}>
        <Bar done={progress.done} total={progress.total} />
      </span>
      <span
        className="num"
        style={{ fontSize: 11.5, color: 'var(--fg-3)', fontVariantNumeric: 'tabular-nums' }}
      >
        {hasTotal ? `${progress.done.toLocaleString()} / ${progress.total.toLocaleString()}` : '…'}
      </span>
    </div>
  );
}

const center: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
  height: '100%',
  padding: 32,
  animation: 'sift-fade-in 240ms var(--ease-out)',
};

const card: React.CSSProperties = {
  width: '100%',
  maxWidth: 380,
  display: 'flex',
  flexDirection: 'column',
  gap: 12,
};

const title: React.CSSProperties = { fontSize: 16, fontWeight: 650, color: 'var(--fg)' };
const sub: React.CSSProperties = { fontSize: 13, lineHeight: 1.5, color: 'var(--fg-3)' };
const count: React.CSSProperties = {
  fontSize: 12,
  color: 'var(--fg-3)',
  fontFamily: 'var(--font-mono)',
  fontVariantNumeric: 'tabular-nums',
};
const retryBtn: React.CSSProperties = {
  alignSelf: 'flex-start',
  height: 32,
  padding: '0 14px',
  borderRadius: 'var(--r-md)',
  border: '1px solid var(--border-strong)',
  background: 'var(--n0)',
  color: 'var(--fg)',
  fontSize: 13,
  cursor: 'pointer',
};
