import React, { useEffect, useState } from 'react';
import { useView } from '../../stores/viewStore';
import { useAccounts } from '../../stores/accountsStore';
import { useSync, outboxTotals } from '../../stores/syncStore';
import { useOutbox } from '../../stores/outboxStore';
import { OutboxPanel } from '../outbox/OutboxPanel';
import { Popover } from '../../ui/Popover';
import { api } from '../../app/ipc/commands';
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
} from 'lucide-react';
import { useLabels } from '../../stores/labelsStore';
import { COLOR_MIX_SUPPORTED } from '../../lib/css';

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

export function Sidebar({ onSettings }: { onSettings: () => void }) {
  const view = useView((s) => s.view);
  const setView = useView((s) => s.setView);
  const accounts = useAccounts((s) => s.accounts);
  const scope = useView((s) => s.accountScope);
  const [labels, setLabels] = useState<Label[]>([]);
  const [counts, setCounts] = useState<Record<string, { unread: number; total: number }>>({});
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(() => {
    try {
      return JSON.parse(localStorage.getItem('sift-label-collapse') ?? '{}');
    } catch {
      return {};
    }
  });

  useEffect(() => {
    const ids = scope === 'all' ? accounts.map((a) => a.id) : [scope];
    if (!ids.length) {
      setLabels([]);
      setCounts({});
      return;
    }
    Promise.all(ids.map((id) => api.labels_list(id).catch(() => [] as Label[]))).then((all) => {
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
          const active = view.kind === v.kind;
          const Icon = v.icon;
          return (
            <button
              key={v.kind}
              onClick={() => setView({ kind: v.kind } as never)}
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
              <Icon size={16} strokeWidth={1.5} color={active ? 'var(--fg)' : 'var(--fg-3)'} />
              <span style={{ flex: 1, textAlign: 'left' }}>{v.label}</span>
              {countFor(v.kind) > 0 && (
                <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-2)' }}>
                  {countFor(v.kind)}
                </span>
              )}
            </button>
          );
        })}
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

function LabelTree({
  labels,
  collapsed,
  onToggle,
  accountHints,
}: {
  labels: Label[];
  collapsed: Record<string, boolean>;
  onToggle: (n: string) => void;
  accountHints: Record<string, string>;
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
}: {
  label: Label;
  onClick: () => void;
  short?: boolean;
  /** Account email, shown only when the same label name exists twice (P3.6). */
  hint?: string;
}) {
  const view = useView((s) => s.view);
  const active = view.kind === 'label' && view.labelId === label.id;
  const name = short ? label.name.split('/').slice(1).join('/') : label.name;
  const title = hint ? `${label.name} — ${hint}` : label.name;
  return (
    <button
      onClick={onClick}
      title={title}
      data-label-name={label.name}
      data-account-id={label.account_id}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        width: '100%',
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
