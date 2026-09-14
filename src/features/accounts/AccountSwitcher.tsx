import React, { useEffect } from 'react';
import { useAccounts } from '../../stores/accountsStore';
import { useView } from '../../stores/viewStore';
import { Avatar } from '../../ui/Avatar';
import { accentHex } from '../../lib/colors';

export function AccountSwitcher() {
  const accounts = useAccounts((s) => s.accounts);
  const scope = useView((s) => s.accountScope);
  const setScope = useView((s) => s.setScope);
  const refresh = useAccounts((s) => s.refresh);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      if (e.key === '0') {
        e.preventDefault();
        setScope('all');
      }
      const n = parseInt(e.key, 10);
      if (n >= 1 && n <= 9 && accounts[n - 1]) {
        e.preventDefault();
        setScope(accounts[n - 1].id);
      }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [accounts, setScope]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  if (accounts.length <= 1) {
    const a = accounts[0];
    if (!a) return null;
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '8px 12px' }}>
        <Avatar email={a.email} name={a.display_name} image={a.avatar_url} />
        <div style={{ minWidth: 0 }}>
          <div
            style={{
              fontSize: 13,
              fontWeight: 600,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {a.display_name ?? a.email}
          </div>
          <div
            style={{
              fontSize: 11.5,
              color: 'var(--fg-3)',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {a.email}
          </div>
        </div>
      </div>
    );
  }

  const selected = accounts.find((a) => a.id === scope);
  const scopeIds: string[] = ['all', ...accounts.map((a) => a.id)];
  const onTabKey = (e: React.KeyboardEvent) => {
    if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
    const i = scopeIds.indexOf(scope === 'all' ? 'all' : scope);
    const at = i < 0 ? 0 : i;
    const next = scopeIds[(at + (e.key === 'ArrowRight' ? 1 : scopeIds.length - 1)) % scopeIds.length];
    e.preventDefault();
    setScope(next === 'all' ? 'all' : next);
    const el = (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-account-tab="${next}"]`);
    el?.focus();
  };

  return (
    <div style={{ padding: '8px 12px' }}>
      <div
        role="tablist"
        aria-label="Accounts"
        onKeyDown={onTabKey}
        style={{ display: 'flex', alignItems: 'center', gap: 6 }}
      >
        <button
          role="tab"
          aria-selected={scope === 'all'}
          tabIndex={scope === 'all' ? 0 : -1}
          data-account-tab="all"
          onClick={() => setScope('all')}
          style={{
            height: 28,
            padding: '0 12px',
            borderRadius: 999,
            border: 'none',
            cursor: 'pointer',
            background: scope === 'all' ? 'var(--accent)' : 'var(--n3)',
            color: scope === 'all' ? 'var(--fg-on-accent)' : 'var(--fg-2)',
            fontSize: 11.5,
            fontWeight: 700,
          }}
          title="All accounts (⌘0)"
        >
          All
        </button>
        {accounts.map((a, i) => {
          const on = scope === a.id;
          return (
            <button
              key={a.id}
              role="tab"
              aria-selected={on}
              tabIndex={on ? 0 : -1}
              data-account-tab={a.id}
              aria-label={`${a.display_name ?? a.email} — ${a.email}`}
              onClick={() => setScope(a.id)}
              title={`${a.email} (⌘${i + 1})`}
              style={{
                position: 'relative',
                padding: 0,
                border: 'none',
                background: 'none',
                cursor: 'pointer',
                borderRadius: '50%',
                boxShadow: on ? `0 0 0 2px var(--bg-sidebar), 0 0 0 4px ${accentHex(a.color)}` : 'none',
                transition: 'box-shadow 120ms var(--ease-out)',
              }}
            >
              <Avatar email={a.email} name={a.display_name} image={a.avatar_url} size={26} />
            </button>
          );
        })}
      </div>
      <div
        style={{
          marginTop: 6,
          fontSize: 11.5,
          color: 'var(--fg-3)',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          whiteSpace: 'nowrap',
        }}
      >
        {scope === 'all' ? `All accounts · ${accounts.length}` : (selected?.email ?? '')}
      </div>
    </div>
  );
}
