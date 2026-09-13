import React, { useEffect, useRef } from 'react';
import {
  cancelSearchTimer,
  scheduleLocalSearch,
  startServerSearch,
  useSearch,
  type SearchOrigin,
} from '../../stores/searchStore';
import { useView } from '../../stores/viewStore';

/**
 * The search field edits the shared search state only (P7.2). It owns no query
 * of its own, so a debounced request can never resurrect a cleared query, and
 * Escape restores the mailbox it was entered from.
 */
export function SearchInput({
  captureOrigin,
  restoreOrigin,
}: {
  captureOrigin: () => SearchOrigin;
  restoreOrigin: (origin: SearchOrigin) => void;
}) {
  const q = useSearch((s) => s.query);
  const scope = useView((s) => s.accountScope);
  const inputRef = useRef<HTMLInputElement>(null);
  const restoreRef = useRef(restoreOrigin);
  restoreRef.current = restoreOrigin;

  // `/` focuses the field from anywhere in the shell.
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

  // A pending debounce must never outlive the field or follow the user into a
  // different account scope.
  useEffect(() => cancelSearchTimer, []);

  const firstScope = useRef(scope);
  useEffect(() => {
    if (firstScope.current === scope) return;
    firstScope.current = scope;
    // The query has not changed, so re-run it against the new account scope
    // instead of leaving a half-applied search behind.
    const search = useSearch.getState();
    if (!search.query.trim()) return;
    if (search.scope === 'server') startServerSearch();
    else scheduleLocalSearch(search.query);
  }, [scope]);

  const leaveSearch = () => {
    cancelSearchTimer();
    const origin = useSearch.getState().leave();
    if (origin) restoreRef.current(origin);
  };

  return (
    <input
      ref={inputRef}
      value={q}
      placeholder="Search ( / )"
      onChange={(e) => {
        const v = e.target.value;
        if (!v.trim()) {
          // Cleared before the debounce settled: nothing may switch the view.
          useSearch.getState().setQuery(v);
          leaveSearch();
          return;
        }
        useSearch.getState().begin(captureOrigin());
        useSearch.getState().setQuery(v);
        scheduleLocalSearch(v);
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          startServerSearch();
          return;
        }
        if (e.key === 'Escape') {
          e.preventDefault();
          leaveSearch();
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
