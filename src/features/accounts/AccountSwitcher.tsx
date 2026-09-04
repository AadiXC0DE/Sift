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

  return (
    <div
      style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '8px 12px' }}
      role="tablist"
      aria-label="Accounts"
    >
      <button
        onClick={() => setScope('all')}
        style={{
          border: scope === 'all' ? '2px solid var(--accent)' : '2px solid transparent',
          borderRadius: '50%',
          width: 30,
          height: 30,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          background: 'var(--n3)',
          cursor: 'pointer',
          fontSize: 11,
          fontWeight: 700,
        }}
        title="All accounts (⌘0)"
      >
        All
      </button>
      {accounts.map((a, i) => (
        <button
          key={a.id}
          onClick={() => setScope(a.id)}
          title={`${a.email} (⌘${i + 1})`}
          style={{
            border: scope === a.id ? `2px solid ${accentHex(a.color)}` : '2px solid transparent',
            borderRadius: '50%',
            padding: 0,
            background: 'none',
            cursor: 'pointer',
          }}
          role="tab"
          aria-selected={scope === a.id}
        >
          <span
            style={{ boxShadow: `0 0 0 2px ${accentHex(a.color)}`, borderRadius: '50%', display: 'block' }}
          >
            <Avatar email={a.email} name={a.display_name} image={a.avatar_url} size={26} />
          </span>
        </button>
      ))}
    </div>
  );
}
