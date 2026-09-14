import React, { useEffect, useState } from 'react';
import { useView } from '../../stores/viewStore';
import { useAccounts } from '../../stores/accountsStore';
import { useSync, outboxTotals } from '../../stores/syncStore';
import { useOutbox } from '../../stores/outboxStore';
import { OutboxPanel } from '../outbox/OutboxPanel';
import { Popover } from '../../ui/Popover';
import { Menu } from '../../ui/Menu';
import { Dialog } from '../../ui/Dialog';
import { Button } from '../../ui/Button';
import { api } from '../../app/ipc/commands';
import { utilities } from '../mail-utilities/ipc';
import { readSiftError } from '../../lib/siftError';
import type { Label } from '../../app/ipc/types';
import { AccountSwitcher } from '../accounts/AccountSwitcher';
import {
  Inbox,
  Star,
  Clock,
  Send,
  FileText,
  Archive,
  Layers,
  AlertOctagon,
  AlertTriangle,
  Trash2,
  Settings,
  CalendarClock,
  BellRing,
  MoreHorizontal,
} from 'lucide-react';
import { useLabels } from '../../stores/labelsStore';
import { COLOR_MIX_SUPPORTED } from '../../lib/css';
import { useScheduled } from '../send-later/scheduledStore';
import { useReminders } from '../reminders/remindersStore';

const views = [
  { kind: 'inbox', label: 'Inbox', icon: Inbox },
  { kind: 'starred', label: 'Starred', icon: Star },
  { kind: 'snoozed', label: 'Snoozed', icon: Clock },
  { kind: 'sent', label: 'Sent', icon: Send },
  { kind: 'drafts', label: 'Drafts', icon: FileText },
  { kind: 'archive', label: 'Archive', icon: Archive },
  // All Mail is a mailbox, not the account scope: it lists every conversation
  // with a message outside Trash/Junk, and the account scope still filters it.
  { kind: 'all_mail', label: 'All Mail', icon: Layers },
  { kind: 'spam', label: 'Spam', icon: AlertOctagon },
  { kind: 'trash', label: 'Trash', icon: Trash2 },
] as const;

export function Sidebar({
  onSettings,
  utility,
  onOpenUtility,
}: {
  onSettings: () => void;
  /** The open Phase 8 panel, so its row can read as the current page. */
  utility: 'send_later' | 'reminders' | null;
  onOpenUtility: (kind: 'send_later' | 'reminders') => void;
}) {
  const view = useView((s) => s.view);
  const setView = useView((s) => s.setView);
  const accounts = useAccounts((s) => s.accounts);
  const scope = useView((s) => s.accountScope);
  // Both rows are conditional on there being something to show (P8.1/P8.2):
  // navigation to an always-empty panel would be a permanent dead end.
  const scheduledCount = useScheduled((s) => s.items.length);
  const reminders = useReminders((s) => s.rows);
  const dueReminders = reminders.filter((r) => r.state === 'due').length;
  const [labels, setLabels] = useState<Label[]>([]);
  const [counts, setCounts] = useState<Record<string, { unread: number; total: number }>>({});
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(() => {
    try {
      return JSON.parse(localStorage.getItem('sift-label-collapse') ?? '{}');
    } catch {
      return {};
    }
  });

  const loadLabels = React.useCallback(() => {
    const ids = scope === 'all' ? accounts.map((a) => a.id) : [scope];
    if (!ids.length) {
      setLabels([]);
      setCounts({});
      return;
    }
    void Promise.all(ids.map((id) => api.labels_list(id).catch(() => [] as Label[]))).then((all) => {
      const merged = all.flat();
      setLabels(merged);
      for (const labels of all) {
        const accountId = labels[0]?.account_id;
        if (accountId) useLabels.getState().apply(accountId, labels);
      }
      const c: Record<string, { unread: number; total: number }> = {};
      for (const l of merged) {
        const e = (c[l.id] ??= { unread: 0, total: 0 });
        e.unread += l.unread_count;
        e.total += l.total_count;
      }
      setCounts(c);
    });
  }, [scope, accounts]);

  useEffect(() => {
    loadLabels();
  }, [loadLabels]);

  const toggleCollapse = (name: string) => {
    setCollapsed((c) => {
      const n = { ...c, [name]: !c[name] };
      localStorage.setItem('sift-label-collapse', JSON.stringify(n));
      return n;
    });
  };

  // One entry per label, keyed by `(account, id)`. Two accounts can each own a
  // label called "Client Work" and they are different labels: merging them by
  // name would navigate to one account's label id for a row that belongs to the
  // other, and would send mutations to the wrong account (P3.6).
  const userLabels = labels.filter((l) => l.kind === 'user' && l.visible);
  // Only disambiguate where the tree would otherwise show one name twice.
  const nameCounts: Record<string, number> = {};
  for (const l of userLabels) nameCounts[l.name] = (nameCounts[l.name] ?? 0) + 1;
  const accountHints: Record<string, string> = {};
  for (const l of userLabels) {
    if (nameCounts[l.name] < 2) continue;
    const email = accounts.find((a) => a.id === l.account_id)?.email;
    if (email) accountHints[`${l.account_id}:${l.id}`] = email;
  }

  // Subtle per-view counts: Inbox and Spam show unread, the rest show totals.
  const countFor = (kind: string): number => {
    const id =
      kind === 'inbox'
        ? 'INBOX'
        : kind === 'starred'
          ? 'STARRED'
          : kind === 'sent'
            ? 'SENT'
            : kind === 'drafts'
              ? 'DRAFT'
              : kind === 'spam'
                ? 'SPAM'
                : kind === 'trash'
                  ? 'TRASH'
                  : '';
    const c = id ? counts[id] : undefined;
    if (!c) return 0;
    return kind === 'inbox' || kind === 'spam' ? c.unread : c.total;
  };

  return (
    <div
      data-tauri-drag-region
      style={{
        width: 'var(--sidebar-w)',
        background: 'var(--bg-sidebar)',
        borderRight: '1px solid var(--border)',
        paddingTop: 'var(--titlebar-h)',
        display: 'flex',
        flexDirection: 'column',
        flexShrink: 0,
        minWidth: 0,
        overflow: 'hidden',
      }}
    >
      <AccountSwitcher />
      <SyncProgress />
      <nav style={{ flex: 1, overflowY: 'auto', padding: '4px 8px' }} aria-label="Mailbox">
        {views.map((v) => {
          const Icon = v.icon;
          return (
            <NavRow
              key={v.kind}
              icon={<Icon size={16} strokeWidth={1.5} />}
              label={v.label}
              count={countFor(v.kind)}
              active={view.kind === v.kind && !utility}
              onClick={() => setView({ kind: v.kind } as never)}
            />
          );
        })}
        {scheduledCount > 0 && (
          <NavRow
            icon={<CalendarClock size={16} strokeWidth={1.5} />}
            label="Send Later"
            count={scheduledCount}
            active={utility === 'send_later'}
            testId="nav-send-later"
            onClick={() => onOpenUtility('send_later')}
          />
        )}
        {reminders.length > 0 && (
          <NavRow
            icon={<BellRing size={16} strokeWidth={1.5} />}
            label="Reminders"
            count={reminders.length}
            warn={dueReminders > 0}
            active={utility === 'reminders'}
            testId="nav-reminders"
            onClick={() => onOpenUtility('reminders')}
          />
        )}
        <div
          style={{
            fontSize: 11,
            color: 'var(--fg-3)',
            padding: '12px 8px 4px',
            textTransform: 'uppercase',
            letterSpacing: '0.04em',
          }}
        >
          Labels
        </div>
        <LabelTree
          labels={userLabels}
          collapsed={collapsed}
          onToggle={toggleCollapse}
          accountHints={accountHints}
          onChanged={loadLabels}
        />
      </nav>
      <PendingFooter />
      <button
        onClick={onSettings}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          height: 36,
          padding: '0 12px',
          background: 'none',
          border: 'none',
          borderTop: '1px solid var(--border)',
          cursor: 'pointer',
          color: 'var(--fg-2)',
          fontSize: 13,
        }}
      >
        <Settings size={16} strokeWidth={1.5} /> Settings
      </button>
    </div>
  );
}

/**
 * One mailbox-style navigation row (P8.1/P8.2). The Phase 8 panels reuse the
 * mailbox rows so they read as views of their own rather than as actions, and
 * so the active state, count and hover behavior stay identical.
 */
function NavRow({
  icon,
  label,
  count,
  active,
  warn,
  testId,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  count: number;
  active: boolean;
  /** Draws the count as a due/undelivered indicator rather than a total. */
  warn?: boolean;
  testId?: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      data-testid={testId}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        width: '100%',
        height: 30,
        padding: '0 8px',
        borderRadius: 'var(--r-md)',
        border: 'none',
        cursor: 'pointer',
        background: active
          ? COLOR_MIX_SUPPORTED
            ? 'color-mix(in oklab, var(--accent) 12%, transparent)'
            : 'var(--bg-row-selected)'
          : undefined,
        color: active ? 'var(--fg)' : 'var(--fg-2)',
        fontSize: 13,
      }}
      className="hoverable"
      aria-current={active ? 'page' : undefined}
    >
      <span style={{ display: 'inline-flex', color: active ? 'var(--fg)' : 'var(--fg-3)' }}>{icon}</span>
      <span style={{ flex: 1, textAlign: 'left' }}>{label}</span>
      {count > 0 && (
        <span className="num" style={{ fontSize: 11.5, color: warn ? 'var(--warning)' : 'var(--fg-2)' }}>
          {count}
        </span>
      )}
    </button>
  );
}

function LabelTree({
  labels,
  collapsed,
  onToggle,
  accountHints,
  onChanged,
}: {
  labels: Label[];
  collapsed: Record<string, boolean>;
  onToggle: (n: string) => void;
  accountHints: Record<string, string>;
  onChanged: () => void;
}) {
  const setView = useView((s) => s.setView);
  // nest by '/'
  const roots: Record<string, Label[]> = {};
  const top: Label[] = [];
  for (const l of labels) {
    const parts = l.name.split('/');
    if (parts.length === 1) top.push(l);
    else {
      const root = parts[0];
      if (!roots[root]) roots[root] = [];
      roots[root].push(l);
    }
  }
  const keyOf = (l: Label) => `${l.account_id}:${l.id}`;
  return (
    <div>
      {top.map((l) => (
        <LabelRow
          key={keyOf(l)}
          label={l}
          hint={accountHints[keyOf(l)]}
          onChanged={onChanged}
          onClick={() => setView({ kind: 'label', labelId: l.id })}
        />
      ))}
      {Object.entries(roots).map(([root, kids]) => (
        <div key={root}>
          <button
            onClick={() => onToggle(root)}
            style={{
              display: 'flex',
              width: '100%',
              background: 'none',
              border: 'none',
              cursor: 'pointer',
              fontSize: 13,
              color: 'var(--fg-2)',
              height: 28,
              alignItems: 'center',
              padding: '0 8px',
            }}
          >
            <span style={{ marginRight: 4 }}>{collapsed[root] ? '▸' : '▾'}</span> {root}
          </button>
          {!collapsed[root] &&
            kids.map((l) => (
              <div key={keyOf(l)} style={{ paddingLeft: 16 }}>
                <LabelRow
                  label={l}
                  hint={accountHints[keyOf(l)]}
                  short
                  onChanged={onChanged}
                  onClick={() => setView({ kind: 'label', labelId: l.id })}
                />
              </div>
            ))}
        </div>
      ))}
    </div>
  );
}

function LabelRow({
  label,
  onClick,
  short,
  hint,
  onChanged,
}: {
  label: Label;
  onClick: () => void;
  short?: boolean;
  /** Account email, shown only when the same label name exists twice (P3.6). */
  hint?: string;
  /** Re-reads this account's labels after a rename or a delete (P8.5). */
  onChanged: () => void;
}) {
  const view = useView((s) => s.view);
  const active = view.kind === 'label' && view.labelId === label.id;
  const name = short ? label.name.split('/').slice(1).join('/') : label.name;
  const title = hint ? `${label.name} — ${hint}` : label.name;
  const [renaming, setRenaming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [draft, setDraft] = useState(label.name);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const rename = async () => {
    const next = draft.trim();
    if (!next || next === label.name) return;
    setBusy(true);
    setError(null);
    try {
      await utilities.label_rename({ accountId: label.account_id, labelId: label.id, name: next });
      setRenaming(false);
      onChanged();
    } catch (e) {
      // The provider's refusal is the honest answer: a duplicate name, a
      // system label, or an offline queue that could not be built.
      setError(readSiftError(e).message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await utilities.label_delete({ accountId: label.account_id, labelId: label.id });
      setDeleting(false);
      if (active) useView.getState().setView({ kind: 'inbox' });
      onChanged();
    } catch (e) {
      setError(readSiftError(e).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="sift-label-row" style={{ display: 'flex', alignItems: 'center' }}>
      <button
        onClick={onClick}
        title={title}
        data-label-name={label.name}
        data-account-id={label.account_id}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          flex: 1,
          minWidth: 0,
          height: 28,
          padding: '0 8px',
          borderRadius: 'var(--r-md)',
          border: 'none',
          cursor: 'pointer',
          background: active
            ? COLOR_MIX_SUPPORTED
              ? 'color-mix(in oklab, var(--accent) 12%, transparent)'
              : 'var(--bg-row-selected)'
            : undefined,
          color: 'var(--fg-2)',
          fontSize: 13,
        }}
        className="hoverable"
      >
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: '50%',
            background: label.color_bg ?? 'var(--accent)',
            flexShrink: 0,
          }}
        />
        <span
          style={{
            flex: 1,
            textAlign: 'left',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {name}
        </span>
        {hint && (
          <span
            style={{
              fontSize: 11,
              color: 'var(--fg-3)',
              flexShrink: 0,
              maxWidth: 96,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {hint}
          </span>
        )}
        {label.unread_count > 0 && (
          <span className="num" style={{ fontSize: 11.5 }}>
            {label.unread_count}
          </span>
        )}
      </button>
      {/*
        Rename and delete (P8.5). The trigger is a real focusable control, so it
        is reachable by keyboard; the row reveals it on hover *or* focus-within
        (P9.5).
      */}
      <span className="sift-label-actions">
        <Menu
          label={hint ? `${label.name} — ${hint}` : label.name}
          trigger={
            <button
              aria-label={`Actions for label ${label.name}`}
              data-testid={`label-menu-${label.account_id}-${label.id}`}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                width: 22,
                height: 22,
                background: 'none',
                border: 'none',
                borderRadius: 5,
                cursor: 'pointer',
                color: 'var(--fg-3)',
              }}
            >
              <MoreHorizontal size={14} />
            </button>
          }
          items={[
            {
              label: 'Rename…',
              action: () => {
                setDraft(label.name);
                setError(null);
                setRenaming(true);
              },
            },
            {
              label: 'Delete label…',
              danger: true,
              action: () => {
                setError(null);
                setDeleting(true);
              },
            },
          ]}
        />
      </span>
      <Dialog open={renaming} onClose={() => setRenaming(false)} title="Rename label" width={400}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            aria-label="Label name"
            data-testid="rename-label-input"
            style={{
              height: 32,
              border: '1px solid var(--border-strong)',
              borderRadius: 6,
              padding: '0 10px',
              background: 'var(--bg-raised)',
              color: 'var(--fg)',
            }}
          />
          <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>
            Nested labels use “/”, for example “Client Work/Invoices”.
          </div>
          {error && (
            <div role="alert" style={{ fontSize: 12.5, color: 'var(--danger)' }}>
              {error}
            </div>
          )}
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
            <Button onClick={() => setRenaming(false)}>Cancel</Button>
            <Button
              variant="primary"
              disabled={busy || !draft.trim() || draft.trim() === label.name}
              onClick={() => void rename()}
            >
              Rename
            </Button>
          </div>
        </div>
      </Dialog>
      <Dialog open={deleting} onClose={() => setDeleting(false)} title="Delete label" width={400}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          <div style={{ fontSize: 13 }}>
            Delete “{label.name}”? This removes the label from its conversations and deletes nothing else: the
            messages stay in All Mail and keep every other label.
          </div>
          {error && (
            <div role="alert" style={{ fontSize: 12.5, color: 'var(--danger)' }}>
              {error}
            </div>
          )}
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
            <Button onClick={() => setDeleting(false)}>Cancel</Button>
            <Button variant="danger" disabled={busy} onClick={() => void remove()}>
              Delete label
            </Button>
          </div>
        </div>
      </Dialog>
    </div>
  );
}

function SyncProgress() {
  const byAccount = useSync((s) => s.byAccount);
  const entries = Object.values(byAccount).filter(
    (s) => s.phase === 'listing' || s.phase === 'metadata' || s.phase === 'full',
  );
  if (!entries.length) return null;
  const s = entries[0];
  const pct = s.total > 0 ? Math.round((s.done / s.total) * 100) : 0;
  return (
    <div style={{ padding: '2px 12px 8px' }} role="status" aria-live="polite">
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          gap: 8,
          fontSize: 11.5,
          color: 'var(--fg-3)',
          marginBottom: 5,
        }}
      >
        <span style={{ whiteSpace: 'nowrap' }}>
          {s.phase === 'metadata' ? 'Downloading mail' : 'Getting your mail'}
        </span>
        {s.total > 0 && (
          <span className="num" style={{ fontVariantNumeric: 'tabular-nums' }}>
            {s.done.toLocaleString()} / {s.total.toLocaleString()}
          </span>
        )}
      </div>
      <div style={{ height: 3, background: 'var(--n3)', borderRadius: 999, overflow: 'hidden' }}>
        <div
          className={s.total ? undefined : 'sync-indeterminate'}
          style={{
            width: s.total ? `${Math.max(3, pct)}%` : '35%',
            height: '100%',
            background: 'var(--accent)',
            borderRadius: 999,
            transition: 'width 240ms var(--ease-out)',
          }}
        />
      </div>
    </div>
  );
}

function PendingFooter() {
  const outbox = useSync((s) => s.outbox);
  const storeOpen = useOutbox((s) => s.open);
  const setOpen = useOutbox((s) => s.setOpen);
  const totals = outboxTotals(outbox);
  const running = totals.pending + totals.inflight;
  const stuck = totals.failed + totals.uncertain;
  if (running + stuck <= 0) return null;
  const details = totals.summary;
  const inline = details
    .slice(0, 2)
    .map((d) => `${d.label}${d.count > 1 ? ` (${d.count})` : ''}`)
    .join(', ');
  const tooltip = stuck
    ? `${stuck} operation${stuck === 1 ? '' : 's'} need attention — open the Outbox`
    : details.length
      ? `Sending your changes to Gmail:\n${details.map((d) => `• ${d.label} (${d.count})`).join('\n')}`
      : 'Sending your changes to Gmail';
  const text = stuck
    ? `${stuck} need${stuck === 1 ? 's' : ''} attention`
    : `Sending ${running.toLocaleString()} change${running === 1 ? '' : 's'} to Gmail`;
  return (
    <Popover
      open={storeOpen}
      onOpenChange={setOpen}
      trigger={
        <button
          title={tooltip}
          role="status"
          aria-live="polite"
          aria-haspopup="dialog"
          data-testid="outbox-indicator"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 7,
            width: '100%',
            padding: '6px 12px',
            borderTop: '1px solid var(--border)',
            border: 'none',
            background: 'none',
            cursor: 'pointer',
            font: 'inherit',
            textAlign: 'left',
            fontSize: 12,
            color: 'var(--fg-2)',
            minWidth: 0,
          }}
        >
          {stuck > 0 ? (
            <AlertTriangle size={12} color="var(--warning)" aria-hidden />
          ) : (
            <span className="spin" aria-hidden style={{ color: 'var(--accent)' }}>
              ◌
            </span>
          )}
          <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {text}
            {inline && !stuck ? `: ${inline}` : ''}
          </span>
        </button>
      }
    >
      <OutboxPanel onClose={() => setOpen(false)} />
    </Popover>
  );
}
