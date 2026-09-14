import React, { useMemo, useState } from 'react';
import { api } from '../../app/ipc/commands';
import type { ConnectivityState } from '../../app/ipc/types';
import { useAccounts } from '../../stores/accountsStore';
import { useSync } from '../../stores/syncStore';
import { relativeTime } from '../../lib/dates';
import { COLOR_MIX_SUPPORTED } from '../../lib/css';
import { AlertTriangle, CloudOff, KeyRound, RefreshCw } from 'lucide-react';

export interface ScopedConnectivityRow {
  accountId: string;
  email: string;
  state: ConnectivityState['state'];
  lastOkAt?: number | null;
  lastError?: string | null;
}

/**
 * Per-account connectivity for the accounts in scope (P4.6).
 *
 * Only accounts that are not plainly online produce a row: a healthy account
 * needs no announcement, and a problem beside a healthy account must not read
 * as a whole-app outage. `lastOkAt` is the newest successful provider
 * operation across the scope, which is what the empty list reports.
 */
export function useScopedConnectivity(accountIds: string[]): {
  rows: ScopedConnectivityRow[];
  lastOkAt: number | null;
} {
  const connectivity = useSync((s) => s.connectivity);
  const accounts = useAccounts((s) => s.accounts);
  const key = accountIds.join(',');
  return useMemo(() => {
    const rows: ScopedConnectivityRow[] = [];
    let lastOkAt: number | null = null;
    for (const id of accountIds) {
      const account = accounts.find((a) => a.id === id);
      const state = connectivity[id];
      const okAt = state?.lastOkAt ?? account?.last_sync_at ?? null;
      if (okAt != null && (lastOkAt == null || okAt > lastOkAt)) lastOkAt = okAt;
      if (!state || state.state === 'online') continue;
      rows.push({
        accountId: id,
        email: account?.email ?? id,
        state: state.state,
        lastOkAt: state.lastOkAt ?? null,
        lastError: state.lastError ?? null,
      });
    }
    return { rows, lastOkAt };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, connectivity, accounts]);
}

const STATE_COPY: Record<ConnectivityState['state'], string> = {
  online: 'Connected',
  offline: 'Offline',
  degraded: 'Having trouble reaching Gmail',
  reauth_required: 'Sign-in needed',
};

/**
 * The inline account state above the list.
 *
 * Cached conversations stay on screen while an account is unreachable: this
 * reports the state, it does not hide rows. Retry re-runs one sync tick for
 * that account; Reconnect hands the account to Settings, which owns the
 * credential flow. Recovery is driven by a provider operation succeeding, so
 * nothing here retries on a timer.
 */
export function ConnectivityStrip({
  accountIds,
  onReconnect,
}: {
  accountIds: string[];
  onReconnect: (accountId: string) => void;
}) {
  const { rows } = useScopedConnectivity(accountIds);
  const [retrying, setRetrying] = useState<string | null>(null);
  if (rows.length === 0) return null;

  const retry = async (accountId: string) => {
    setRetrying(accountId);
    try {
      await api.sync_now(accountId);
    } catch {
      // Still unreachable: the account keeps its error until an operation succeeds.
    } finally {
      setRetrying(null);
    }
  };

  return (
    <div
      data-testid="connectivity-strip"
      style={{
        borderBottom: '1px solid var(--border)',
        background: COLOR_MIX_SUPPORTED ? 'color-mix(in oklab, var(--warning) 8%, transparent)' : 'var(--n1)',
        padding: '6px 12px',
        display: 'flex',
        flexDirection: 'column',
        gap: 4,
        flexShrink: 0,
      }}
    >
      {rows.map((r) => {
        const Icon =
          r.state === 'reauth_required' ? KeyRound : r.state === 'degraded' ? AlertTriangle : CloudOff;
        return (
          <div
            key={r.accountId}
            data-account-id={r.accountId}
            data-state={r.state}
            style={{
              display: 'grid',
              gridTemplateColumns: '14px minmax(0, 1fr) auto',
              alignItems: 'center',
              gap: '4px 8px',
              minWidth: 0,
              fontSize: 12,
            }}
          >
            <Icon size={14} color="var(--warning)" style={{ flexShrink: 0 }} />
            <span
              style={{
                color: 'var(--fg-2)',
                minWidth: 0,
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
              }}
              title={r.email}
            >
              {r.email}
            </span>
            <span
              role="status"
              aria-live="polite"
              style={{
                gridColumn: '2 / -1',
                gridRow: 2,
                display: 'flex',
                flexDirection: 'column',
                gap: 2,
                minWidth: 0,
              }}
            >
              <span style={{ color: 'var(--warning)', flexShrink: 0 }}>{STATE_COPY[r.state]}</span>
              <span
                style={{
                  color: 'var(--fg-3)',
                  flex: 1,
                  minWidth: 0,
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {r.lastError ? `${r.lastError} · ` : ''}
                {r.lastOkAt ? `Last synced ${relativeTime(r.lastOkAt)}` : 'Not synced yet'}
              </span>
            </span>
            {r.state === 'reauth_required' ? (
              <button
                className="sift-chip-btn"
                style={{ gridColumn: 3, gridRow: 1 }}
                data-testid="connectivity-reconnect"
                onClick={() => onReconnect(r.accountId)}
              >
                Reconnect
              </button>
            ) : (
              <button
                className="sift-chip-btn"
                style={{ gridColumn: 3, gridRow: 1 }}
                data-testid="connectivity-retry"
                aria-busy={retrying === r.accountId}
                onClick={() => void retry(r.accountId)}
              >
                <RefreshCw size={12} style={{ marginRight: 4 }} /> Retry
              </button>
            )}
          </div>
        );
      })}
      <span style={{ fontSize: 11.5, color: 'var(--fg-3)' }}>
        Cached conversations are still available and your changes will sync when the account is back.
      </span>
    </div>
  );
}
