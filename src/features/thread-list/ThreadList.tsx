import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useThreadsWindow } from './useThreadsWindow';
import { ThreadRowView } from './ThreadRow';
import { useView } from '../../stores/viewStore';
import { useSelection } from '../../stores/selectionStore';
import { useAccounts } from '../../stores/accountsStore';
import { useSettings } from '../../stores/settingsStore';
import { viewTitle } from '../../app/routes';
import { Spinner } from '../../ui/Spinner';
import { EmptyState } from '../../ui/EmptyState';
import { Skeleton } from '../../ui/Skeleton';
import { SearchInput } from '../search/SearchInput';
import { dispatchAction } from '../actions/dispatch';
import { Sun } from 'lucide-react';

export function ThreadList({ onCompose }: { onCompose: () => void }) {
  void onCompose;
  const view = useView((s) => s.view);
  const scope = useView((s) => s.accountScope);
  const setThread = useView((s) => s.setThread);
  const threadId = useView((s) => s.threadId);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [hasAtt, setHasAtt] = useState(false);
  const { rows, loading, loadMore } = useThreadsWindow(view, { unreadOnly, hasAttachment: hasAtt });
  const focusedIndex = useSelection((s) => s.focusedIndex);
  const setFocus = useSelection((s) => s.setFocus);
  const selectedIds = useSelection((s) => s.selectedIds);
  const accounts = useAccounts((s) => s.accounts);
  const density = useSettings((s) => s.settings.density);
  const rowH = density === 'compact' ? 32 : density === 'comfortable' ? 48 : 40;
  const parentRef = useRef<HTMLDivElement>(null);
  const [searching, setSearching] = useState(false);

  const colorOf = useCallback(
    (accountId: string) => accounts.find((a) => a.id === accountId)?.color ?? 'blue',
    [accounts],
  );
  const showStripe = scope === 'all' && accounts.length > 1;

  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowH,
    overscan: 8,
  });

  // keyboard j/k/x etc for list scope
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (view.kind === 'search' && searching) return;
      if (e.key === 'j' || e.key === 'ArrowDown') {
        e.preventDefault();
        setFocus(Math.min(rows.length - 1, focusedIndex + 1));
      } else if (e.key === 'k' || e.key === 'ArrowUp') {
        e.preventDefault();
        setFocus(Math.max(0, focusedIndex - 1));
      } else if (e.key === 'x') {
        const r = rows[focusedIndex];
        if (r) useSelection.getState().toggle(`${r.accountId}:${r.id}`);
      } else if (e.key === 'e') {
        const r = rows[focusedIndex];
        if (r)
          void dispatchAction({ accountId: r.accountId, threadIds: [r.id], action: { kind: 'archive' } });
      } else if (e.key === 'Enter' || e.key === 'o') {
        const r = rows[focusedIndex];
        if (r) setThread(r.id);
      }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [rows, focusedIndex, setFocus, setThread, view.kind, searching]);

  // keep focused visible
  useEffect(() => {
    virtual.scrollToIndex(focusedIndex, { align: 'auto' });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusedIndex]);

  const title = useMemo(() => {
    if (view.kind === 'label') return (view as { labelId: string }).labelId;
    if (view.kind === 'search') return `Results for “${(view as { q: string }).q}”`;
    return viewTitle[view.kind] ?? 'Inbox';
  }, [view]);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}>
      <div
        style={{
          height: 'var(--toolbar-h)',
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          padding: '0 12px',
          borderBottom: '1px solid var(--border)',
          flexShrink: 0,
        }}
      >
        <span style={{ fontWeight: 600, fontSize: 14, flex: 1 }}>{title}</span>
        <SearchInput onSearching={setSearching} />
        <button
          onClick={() => setUnreadOnly((v) => !v)}
          title="Unread only"
          style={{
            fontSize: 12,
            background: unreadOnly ? 'var(--accent-soft)' : 'none',
            border: '1px solid var(--border)',
            borderRadius: 6,
            padding: '2px 8px',
            cursor: 'pointer',
          }}
        >
          Unread
        </button>
        <button
          onClick={() => setHasAtt((v) => !v)}
          title="Has attachment"
          style={{
            fontSize: 12,
            background: hasAtt ? 'var(--accent-soft)' : 'none',
            border: '1px solid var(--border)',
            borderRadius: 6,
            padding: '2px 8px',
            cursor: 'pointer',
          }}
        >
          📎
        </button>
      </div>
      <div
        ref={parentRef}
        style={{ flex: 1, overflowY: 'auto', position: 'relative' }}
        role="listbox"
        aria-label={title}
      >
        {loading && rows.length === 0 ? (
          <DelayedSkeleton />
        ) : rows.length === 0 ? (
          view.kind === 'inbox' ? (
            <div style={{ animation: 'sift-fade 400ms var(--ease-out)' }}>
              <style>{'@keyframes sift-fade { from { opacity: 0; } }'}</style>
              <EmptyState icon={<Sun size={24} />} line="You're all caught up." sub="Last synced just now" />
            </div>
          ) : (
            <EmptyState
              line="No conversations match."
              sub={view.kind === 'search' ? 'Search Gmail instead ↩' : undefined}
            />
          )
        ) : (
          <div style={{ height: virtual.getTotalSize(), position: 'relative' }}>
            {virtual.getVirtualItems().map((vi) => {
              const r = rows[vi.index];
              if (!r) return null;
              const key = `${r.accountId}:${r.id}`;
              return (
                <div
                  key={key}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    transform: `translateY(${vi.start}px)`,
                  }}
                >
                  <ThreadRowView
                    row={r}
                    focused={vi.index === focusedIndex}
                    selected={selectedIds.has(key)}
                    accountColor={colorOf(r.accountId)}
                    showStripe={showStripe}
                    onFocus={() => setFocus(vi.index)}
                    onToggleSelect={() => useSelection.getState().toggle(key)}
                    onOpen={() => setThread(r.id)}
                  />
                </div>
              );
            })}
          </div>
        )}
        {!loading && <ScrollSentinel onVisible={loadMore} />}
        {loading && rows.length > 0 && (
          <div style={{ padding: 8, display: 'flex', justifyContent: 'center' }}>
            <Spinner size={12} />
          </div>
        )}
      </div>
      {threadId && <div style={{ display: 'none' }}>{threadId}</div>}
    </div>
  );
}

function DelayedSkeleton() {
  const [show, setShow] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setShow(true), 120);
    return () => clearTimeout(t);
  }, []);
  if (!show) return null;
  return (
    <div style={{ padding: 12, display: 'flex', flexDirection: 'column', gap: 8 }}>
      <Skeleton />
      <Skeleton />
      <Skeleton />
    </div>
  );
}

function ScrollSentinel({ onVisible }: { onVisible: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver((es) => {
      if (es[0].isIntersecting) onVisible();
    });
    io.observe(el);
    return () => io.disconnect();
  }, [onVisible]);
  return <div ref={ref} style={{ height: 1 }} />;
}
