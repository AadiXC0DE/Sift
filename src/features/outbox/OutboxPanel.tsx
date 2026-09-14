import React from 'react';
import { AlertTriangle, Check, Clock, Loader, Send, X } from 'lucide-react';
import type { OutboxOp, OperationState } from '../../app/ipc/types';
import { useAccounts } from '../../stores/accountsStore';
import { useOutbox, OUTBOX_PAGE_LIMIT } from '../../stores/outboxStore';
import { formatRowDate } from '../../lib/dates';

/**
 * The compact Outbox (P6.6).
 *
 * Every value this panel renders is a lightweight summary the backend composed
 * at enqueue time — recipient summary, locally known subject, action, schedule
 * or retry time and the error. The raw MIME payload never crosses this boundary
 * and never appears in the DOM; the panel has no field to put it in.
 */
export function OutboxPanel({ onClose }: { onClose?: () => void }) {
  const accounts = useAccounts((s) => s.accounts);
  const included = useAccounts((s) => s.included);
  const operations = useOutbox((s) => s.operations);
  const counts = useOutbox((s) => s.counts);
  const total = useOutbox((s) => s.total);
  const nextCursor = useOutbox((s) => s.nextCursor);
  const loading = useOutbox((s) => s.loading);
  const error = useOutbox((s) => s.error);
  const refresh = useOutbox((s) => s.refresh);
  const loadMore = useOutbox((s) => s.loadMore);

  const accountIds = React.useMemo(
    () => accounts.filter((a) => included[a.id] !== false).map((a) => a.id),
    [accounts, included],
  );
  const idsRef = React.useRef(accountIds);
  idsRef.current = accountIds;
  // The effect keys off the account *set*, not the array identity: a store that
  // hands back a fresh array each read must not restart the fetch forever.
  const accountKey = accountIds.join('|');

  React.useEffect(() => {
    void refresh(idsRef.current);
  }, [accountKey, refresh]);

  const needsYou = counts.failed + counts.uncertain;
  const head =
    counts.pending + counts.inflight === 0
      ? needsYou > 0
        ? `${needsYou} need${needsYou === 1 ? 's' : ''} attention`
        : 'Nothing is waiting'
      : `${counts.pending + counts.inflight} waiting`;

  return (
    <div
      role="region"
      aria-label="Outbox"
      data-testid="outbox-panel"
      style={{ width: 380, maxWidth: '86vw', display: 'flex', flexDirection: 'column', gap: 6 }}
    >
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 8, padding: '2px 6px 6px' }}>
        <strong style={{ fontSize: 13 }}>Outbox</strong>
        <span style={{ fontSize: 11.5, color: 'var(--fg-3)' }}>{head}</span>
        <span style={{ flex: 1 }} />
        {onClose && (
          <button
            onClick={onClose}
            aria-label="Close outbox"
            style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--fg-3)' }}
          >
            <X size={14} />
          </button>
        )}
      </div>

      {error && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--danger)', padding: '0 6px' }}>
          {error}
        </div>
      )}

      <div role="list" style={{ overflowY: 'auto', maxHeight: '52vh', display: 'grid', gap: 2 }}>
        {operations.length === 0 && !loading && (
          <div style={{ fontSize: 12.5, color: 'var(--fg-3)', padding: '10px 6px' }}>
            Nothing is queued. Mail you send leaves the queue here once the provider accepts it.
          </div>
        )}
        {operations.map((op) => (
          <OutboxRow key={op.opId} op={op} />
        ))}
      </div>

      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          borderTop: '1px solid var(--border)',
          padding: '6px 6px 2px',
          fontSize: 11.5,
          color: 'var(--fg-3)',
        }}
      >
        {/* The count is the honest size of the queue; the list stays one page. */}
        <span>
          {operations.length === 0
            ? 'No operations'
            : `Showing ${operations.length} of ${total.toLocaleString()}`}
        </span>
        <span style={{ flex: 1 }} />
        {counts.done > 0 && <span>{counts.done.toLocaleString()} completed</span>}
        {nextCursor && (
          <button
            onClick={() => void loadMore(idsRef.current)}
            disabled={loading}
            style={{ fontSize: 11.5, background: 'none', border: 'none', cursor: 'pointer' }}
          >
            {loading ? 'Loading…' : `Load ${OUTBOX_PAGE_LIMIT} more`}
          </button>
        )}
      </div>
    </div>
  );
}

const STATE_LABEL: Record<OperationState, string> = {
  pending: 'Queued',
  inflight: 'Sending',
  uncertain: 'Unconfirmed',
  done: 'Sent',
  failed: 'Failed',
  cancelled: 'Cancelled',
};

function stateLabel(op: OutboxOp): string {
  if (op.kind !== 'send' && op.state === 'inflight') return 'Syncing';
  if (op.kind !== 'send' && op.state === 'done') return 'Completed';
  return STATE_LABEL[op.state];
}

function stateIcon(state: OperationState): React.ReactNode {
  if (state === 'failed') return <AlertTriangle size={12} color="var(--danger)" />;
  if (state === 'uncertain') return <AlertTriangle size={12} color="var(--warning)" />;
  if (state === 'done') return <Check size={12} />;
  if (state === 'inflight') return <Loader size={12} />;
  if (state === 'pending') return <Clock size={12} />;
  return <X size={12} />;
}

/**
 * When the operation will happen, stated from what is actually known: a
 * scheduled send has a time, a retry has the next attempt, and anything else is
 * described by its state rather than by an invented estimate.
 */
function whenText(op: OutboxOp, now: number): string {
  if (op.scheduledAt && op.scheduledAt > now) return `Scheduled for ${formatRowDate(op.scheduledAt, now)}`;
  if (op.retryAt && op.retryAt > now) return `Retries ${formatRowDate(op.retryAt, now)}`;
  if (op.state === 'inflight') return op.kind === 'send' ? 'Sending now' : 'Syncing with Gmail';
  if (op.state === 'uncertain') return 'Checking the provider for a sent copy';
  if (op.state === 'failed') return 'Needs retry';
  if (op.state === 'pending') return op.kind === 'send' ? 'Waiting to send' : 'Waiting to sync';
  return stateLabel(op);
}

function OutboxRow({ op }: { op: OutboxOp }) {
  const acknowledged = useOutbox((s) => s.acknowledged[op.opId] === true);
  const setAcknowledged = useOutbox((s) => s.setAcknowledged);
  const retry = useOutbox((s) => s.retry);
  const now = Date.now();
  const scheduled = op.state === 'pending' && !!op.scheduledAt && op.scheduledAt > now;
  const retryable = op.state === 'failed' || op.state === 'uncertain';
  // An uncertain send may already be in the provider's queue. Retrying is the
  // user's decision, made with the risk stated in plain words (P6.6); the
  // button refuses until that sentence is acknowledged.
  const needsAck = op.state === 'uncertain';

  return (
    <div
      role="listitem"
      data-testid={`outbox-row-${op.opId}`}
      data-state={op.state}
      style={{
        display: 'grid',
        gap: 2,
        padding: '7px 6px',
        borderBottom: '1px solid var(--border)',
        fontSize: 12.5,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        {stateIcon(op.state)}
        <span
          style={{
            fontWeight: 550,
            minWidth: 0,
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {op.recipientSummary || op.subject || op.action}
        </span>
        <span style={{ flex: 1 }} />
        <span
          data-testid={`outbox-state-${op.opId}`}
          style={{ fontSize: 11, color: 'var(--fg-3)', whiteSpace: 'nowrap' }}
        >
          {scheduled ? 'Scheduled' : stateLabel(op)}
        </span>
      </div>
      {op.recipientSummary && op.subject && (
        <div
          style={{
            color: 'var(--fg-2)',
            minWidth: 0,
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {op.subject}
        </div>
      )}
      <div style={{ display: 'flex', gap: 6, color: 'var(--fg-3)', fontSize: 11.5 }}>
        {(op.recipientSummary || op.subject) && (
          <>
            <span>{op.action}</span>
            <span>·</span>
          </>
        )}
        <span>{whenText(op, now)}</span>
      </div>
      {op.errorMessage && op.state !== 'failed' && (
        <div style={{ color: 'var(--fg-3)', overflowWrap: 'anywhere' }}>{op.errorMessage}</div>
      )}
      {op.state === 'failed' && op.errorMessage && (
        <div role="note" style={{ color: 'var(--danger)' }}>
          {op.errorMessage}
        </div>
      )}
      {retryable && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 2 }}>
          {needsAck && (
            <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 11.5 }}>
              <input
                type="checkbox"
                checked={acknowledged}
                onChange={(e) => setAcknowledged(op.opId, e.target.checked)}
                data-testid={`outbox-ack-${op.opId}`}
              />
              Retry sending — may duplicate
            </label>
          )}
          <span style={{ flex: 1 }} />
          <button
            data-testid={`outbox-retry-${op.opId}`}
            disabled={needsAck && !acknowledged}
            onClick={() => void retry(op.opId, needsAck)}
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 4,
              fontSize: 11.5,
              padding: '3px 8px',
              borderRadius: 6,
              border: '1px solid var(--border)',
              background: 'none',
              cursor: needsAck && !acknowledged ? 'not-allowed' : 'pointer',
              opacity: needsAck && !acknowledged ? 0.5 : 1,
            }}
          >
            <Send size={12} /> Retry
          </button>
        </div>
      )}
    </div>
  );
}
