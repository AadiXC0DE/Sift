import React, { useEffect, useState } from 'react';
import { useView } from '../../stores/viewStore';
import { useAccounts } from '../../stores/accountsStore';
import { useSync } from '../../stores/syncStore';
import { api } from '../../app/ipc/commands';
import type { Label } from '../../app/ipc/types';
import { AccountSwitcher } from '../accounts/AccountSwitcher';
import { Inbox, Star, Clock, Send, FileText, Archive, AlertOctagon, Trash2, Settings } from 'lucide-react';

const views = [
  { kind: 'inbox', label: 'Inbox', icon: Inbox },
  { kind: 'starred', label: 'Starred', icon: Star },
  { kind: 'snoozed', label: 'Snoozed', icon: Clock },
  { kind: 'sent', label: 'Sent', icon: Send },
  { kind: 'drafts', label: 'Drafts', icon: FileText },
  { kind: 'archive', label: 'Archive', icon: Archive },
  { kind: 'spam', label: 'Spam', icon: AlertOctagon },
  { kind: 'trash', label: 'Trash', icon: Trash2 },
] as const;

export function Sidebar({ onSettings }: { onSettings: () => void }) {
  const view = useView((s) => s.view);
  const setView = useView((s) => s.setView);
  const accounts = useAccounts((s) => s.accounts);
  const scope = useView((s) => s.accountScope);
  const [labels, setLabels] = useState<Label[]>([]);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(() => {
    try {
      return JSON.parse(localStorage.getItem('sift-label-collapse') ?? '{}');
    } catch {
      return {};
    }
  });

  useEffect(() => {
    const aid = scope === 'all' ? accounts[0]?.id : scope;
    if (!aid) {
      setLabels([]);
      return;
    }
    api
      .labels_list(aid)
      .then(setLabels)
      .catch(() => {});
  }, [scope, accounts]);

  const toggleCollapse = (name: string) => {
    setCollapsed((c) => {
      const n = { ...c, [name]: !c[name] };
      localStorage.setItem('sift-label-collapse', JSON.stringify(n));
      return n;
    });
  };

  const userLabels = labels.filter((l) => l.kind === 'user' && l.visible);
  const unreadFor = (kind: string): number => {
    if (scope !== 'all') {
      const l = labels.find((x) => x.id.toLowerCase() === kind || x.name.toLowerCase() === kind);
      return l?.unread_count ?? 0;
    }
    return 0; // unified sums computed via labels_list per account in real impl; keep 0 for stub
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
                background: active ? 'color-mix(in oklab, var(--accent) 12%, transparent)' : undefined,
                color: active ? 'var(--fg)' : 'var(--fg-2)',
                fontSize: 13,
              }}
              className="hoverable"
              aria-current={active ? 'page' : undefined}
            >
              <Icon size={16} strokeWidth={1.5} color={active ? 'var(--fg)' : 'var(--fg-3)'} />
              <span style={{ flex: 1, textAlign: 'left' }}>{v.label}</span>
              {unreadFor(v.kind) > 0 && (
                <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-3)' }}>
                  {unreadFor(v.kind)}
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
        <LabelTree labels={userLabels} collapsed={collapsed} onToggle={toggleCollapse} />
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
}: {
  labels: Label[];
  collapsed: Record<string, boolean>;
  onToggle: (n: string) => void;
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
  return (
    <div>
      {top.map((l) => (
        <LabelRow key={l.id} label={l} onClick={() => setView({ kind: 'label', labelId: l.id })} />
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
              <div key={l.id} style={{ paddingLeft: 16 }}>
                <LabelRow label={l} short onClick={() => setView({ kind: 'label', labelId: l.id })} />
              </div>
            ))}
        </div>
      ))}
    </div>
  );
}

function LabelRow({ label, onClick, short }: { label: Label; onClick: () => void; short?: boolean }) {
  const view = useView((s) => s.view);
  const active = view.kind === 'label' && (view as { labelId?: string }).labelId === label.id;
  const name = short ? label.name.split('/').slice(1).join('/') : label.name;
  return (
    <button
      onClick={onClick}
      title={label.name}
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
        background: active ? 'color-mix(in oklab, var(--accent) 12%, transparent)' : undefined,
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
  const pending = useSync((s) => s.pending);
  const summaries = useSync((s) => s.pendingSummary);
  const total = Object.values(pending).reduce((a, b) => a + b, 0);
  if (total <= 0) return null;
  const details = Object.values(summaries).flat();
  const inline = details
    .slice(0, 2)
    .map((d) => `${d.label}${d.count > 1 ? ` (${d.count})` : ''}`)
    .join(', ');
  const tooltip = details.length
    ? `Sending your changes to Gmail:\n${details.map((d) => `• ${d.label} (${d.count})`).join('\n')}`
    : 'Sending your changes to Gmail';
  return (
    <div
      title={tooltip}
      role="status"
      aria-live="polite"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 7,
        padding: '6px 12px',
        borderTop: '1px solid var(--border)',
        fontSize: 12,
        color: 'var(--fg-2)',
        minWidth: 0,
      }}
    >
      <span className="spin" aria-hidden style={{ color: 'var(--accent)' }}>
        ◌
      </span>
      <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        Sending {total} change{total === 1 ? '' : 's'} to Gmail
        {inline ? `: ${inline}` : ''}
      </span>
    </div>
  );
}
