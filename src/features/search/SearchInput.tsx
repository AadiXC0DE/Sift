import React, { useEffect, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import { useView } from '../../stores/viewStore';
import { useAccounts } from '../../stores/accountsStore';
import { debounce } from '../../lib/debounce';

export function SearchInput({ onSearching }: { onSearching: (v: boolean) => void }) {
  const [q, setQ] = useState('');
  const view = useView((s) => s.view);
  const setView = useView((s) => s.setView);
  const scope = useView((s) => s.accountScope);
  const includedIds = useAccounts((s) => s.includedIds);
  const inputRef = useRef<HTMLInputElement>(null);
  const stash = useRef<{ view: typeof view; scroll: number } | null>(null);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === '/' && !e.metaKey && !e.ctrlKey) {
        const t = e.target as HTMLElement | null;
        if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
        e.preventDefault();
        inputRef.current?.focus();
      }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, []);

  const doLocal = React.useMemo(
    () =>
      debounce(async (query: string) => {
        if (!query) {
          if (stash.current) {
            // Esc path restores; empty query returns to inbox? keep search view cleared
          }
          onSearching(false);
          return;
        }
        onSearching(true);
        const ids = scope === 'all' ? includedIds() : [scope];
        // local search replaces list via view state search
        setView({ kind: 'search', q: query });
        // warm local results (ThreadList uses view search -> threads_query search branch)
        void api.search(ids, query, 'local').catch(() => {});
      }, 40),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [scope],
  );

  return (
    <input
      ref={inputRef}
      value={q}
      placeholder="Search ( / )"
      onChange={(e) => {
        const v = e.target.value;
        setQ(v);
        if (!stash.current) stash.current = { view, scroll: window.scrollY };
        if (!v) {
          useView.getState().restore();
          onSearching(false);
          return;
        }
        doLocal(v);
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          const ids = scope === 'all' ? includedIds() : [scope];
          void api.search(ids, q, 'server').catch(() => {});
          // save recent
          try {
            const raw = localStorage.getItem('sift-recent-search') ?? '[]';
            const arr: string[] = JSON.parse(raw);
            localStorage.setItem(
              'sift-recent-search',
              JSON.stringify([q, ...arr.filter((x) => x !== q)].slice(0, 10)),
            );
          } catch {
            /* noop */
          }
        }
        if (e.key === 'Escape') {
          setQ('');
          useView.getState().restore();
          onSearching(false);
          (e.target as HTMLInputElement).blur();
        }
      }}
      style={{
        width: 140,
        height: 28,
        fontSize: 13,
        border: '1px solid var(--border)',
        borderRadius: 6,
        padding: '0 8px',
        background: 'var(--n0)',
        color: 'var(--fg)',
      }}
      aria-label="Search"
    />
  );
}
