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
import { dispatchAction, undoLast } from '../actions/dispatch';
import { api } from '../../app/ipc/commands';
import { Popover } from '../../ui/Popover';
import { LabelPicker } from '../actions/LabelPicker';
import { toast } from 'sonner';
import type { Label } from '../../app/ipc/types';
import { Paperclip, Sun } from 'lucide-react';

export function ThreadList({ onCompose }: { onCompose: () => void }) {
  void onCompose;
  const view = useView((s) => s.view);
  const scope = useView((s) => s.accountScope);
  const setThread = useView((s) => s.setThread);
  const threadId = useView((s) => s.threadId);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [hasAtt, setHasAtt] = useState(false);
  const [pickerHost, setPickerHost] = useState<null | {
    kind: 'label' | 'move';
    accountId: string;
    threadIds: string[];
  }>(null);
  const [hostLabels, setHostLabels] = useState<Label[]>([]);
  const [labelName, setLabelName] = useState<string | null>(null);
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

  // Group the selection by account for bulk actions (spec 3.4.5); falls back
  // to the focused row for single actions.
  const targets = (): { accountId: string; threadIds: string[] }[] => {
    const sel = [...useSelection.getState().selectedIds];
    if (sel.length > 1) {
      const byAcc: Record<string, string[]> = {};
      for (const k of sel) {
        const i = k.indexOf(':');
        if (i < 0) continue;
        (byAcc[k.slice(0, i)] ??= []).push(k.slice(i + 1));
      }
      return Object.entries(byAcc).map(([accountId, threadIds]) => ({ accountId, threadIds }));
    }
    const r = rows[useSelection.getState().focusedIndex];
    return r ? [{ accountId: r.accountId, threadIds: [r.id] }] : [];
  };

  const openRow = (index: number) => {
    const r = rows[index];
    if (!r) return;
    (window as unknown as { __lastAccount?: string }).__lastAccount = r.accountId;
    setThread(r.id);
  };

  const moveCursor = (d: number) => {
    const n = Math.max(0, Math.min(rows.length - 1, focusedIndex + d));
    setFocus(n);
    // The reading pane follows the keyboard cursor (spec 12.2).
    if (useView.getState().paneLayout !== 'off') {
      const r = rows[n];
      if (r) {
        (window as unknown as { __lastAccount?: string }).__lastAccount = r.accountId;
        setThread(r.id);
      }
    }
  };

  const snoozeTomorrow = (accountId: string, threadIds: string[]) => {
    const d = new Date();
    d.setDate(d.getDate() + 1);
    d.setHours(8, 0, 0, 0);
    void api.snooze_set(accountId, threadIds, d.getTime()).then(() => {
      toast('Snoozed until Tomorrow 08:00', {
        action: { label: 'Undo (z)', onClick: () => void undoLast() },
      });
    });
  };

  // keyboard for list scope (capture so j/k beat typeahead-find and iframes)
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const w = window as unknown as { __paletteOpen?: boolean; __composeOpen?: boolean };
      if (w.__paletteOpen || w.__composeOpen) return;
      if (view.kind === 'search' && searching) return;
      const k = e.key;
      if (k === 'j' || k === 'ArrowDown') {
        e.preventDefault();
        e.stopPropagation();
        moveCursor(1);
      } else if (k === 'k' || k === 'ArrowUp') {
        e.preventDefault();
        e.stopPropagation();
        moveCursor(-1);
      } else if (k === 'x') {
        const r = rows[focusedIndex];
        if (r) useSelection.getState().toggle(`${r.accountId}:${r.id}`);
      } else if (k === 'Enter' || (k === 'o' && useView.getState().threadId == null)) {
        openRow(focusedIndex);
      } else if (useView.getState().threadId != null) {
        return; // open thread owns e/#//!/s/u/i/h/l/v
      } else if (k === 'e') {
        for (const g of targets()) void dispatchAction({ ...g, action: { kind: 'archive' } });
      } else if (k === '#') {
        for (const g of targets()) void dispatchAction({ ...g, action: { kind: 'trash' } });
      } else if (k === '!') {
        for (const g of targets()) void dispatchAction({ ...g, action: { kind: 'spam' } });
      } else if (k === 's') {
        const r = rows[focusedIndex];
        if (r)
          void dispatchAction({
            accountId: r.accountId,
            threadIds: [r.id],
            action: { kind: 'star', on: !r.isStarred },
          });
      } else if (k === 'U') {
        for (const g of targets()) void dispatchAction({ ...g, action: { kind: 'read', on: false } });
      } else if (k === 'I') {
        for (const g of targets()) void dispatchAction({ ...g, action: { kind: 'read', on: true } });
      } else if (k === 'h') {
        const g = targets()[0];
        if (g) snoozeTomorrow(g.accountId, g.threadIds);
      } else if (k === 'l' || k === 'v') {
        const g = targets()[0];
        if (g) setPickerHost({ kind: k === 'l' ? 'label' : 'move', ...g });
      }
    };
    window.addEventListener('keydown', h, true);
    return () => window.removeEventListener('keydown', h, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows, focusedIndex, setFocus, setThread, view.kind, searching, selectedIds]);

  // Archive-and-go (]/[ from the thread view): archive happened there; advance here.
  useEffect(() => {
    const h = (e: Event) => {
      const dir = (e as CustomEvent).detail?.dir as 1 | -1;
      const cur = useSelection.getState().focusedIndex;
      const next = Math.max(0, Math.min(rows.length - 1, cur + dir));
      setFocus(next);
      if (useView.getState().paneLayout !== 'off') openRow(next);
    };
    document.addEventListener('sift:archive-nav', h as EventListener);
    return () => document.removeEventListener('sift:archive-nav', h as EventListener);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows, setThread]);

  useEffect(() => {
    if (pickerHost) {
      api
        .labels_list(pickerHost.accountId)
        .then(setHostLabels)
        .catch(() => setHostLabels([]));
    }
  }, [pickerHost]);

  useEffect(() => {
    if (view.kind !== 'label') {
      setLabelName(null);
      return;
    }
    const id = (view as { labelId: string }).labelId;
    const aids = scope === 'all' ? accounts.map((a) => a.id) : [scope];
    Promise.all(aids.map((a) => api.labels_list(a).catch(() => [] as Label[]))).then((all) => {
      const found = all.flat().find((l) => l.id === id);
      setLabelName(found ? (found.name.split('/').pop() ?? found.name) : id);
    });
  }, [view, scope, accounts]);

  // keep focused visible
  useEffect(() => {
    virtual.scrollToIndex(focusedIndex, { align: 'auto' });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusedIndex]);

  const title = useMemo(() => {
    if (view.kind === 'label') return labelName ?? (view as { labelId: string }).labelId;
    if (view.kind === 'search') return `Results for “${(view as { q: string }).q}”`;
    return viewTitle[view.kind] ?? 'Inbox';
  }, [view, labelName]);

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
          minWidth: 0,
          overflow: 'hidden',
        }}
      >
        <span
          style={{
            fontWeight: 600,
            fontSize: 14,
            flex: 1,
            minWidth: 0,
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {title}
        </span>
        <SearchInput onSearching={setSearching} />
        <button
          onClick={() => setUnreadOnly((v) => !v)}
          title="Unread only"
          aria-pressed={unreadOnly}
          className="sift-chip-btn"
        >
          Unread
        </button>
        <button
          onClick={() => setHasAtt((v) => !v)}
          title="Has attachment"
          aria-pressed={hasAtt}
          className="sift-chip-btn"
          aria-label="Has attachment"
        >
          <Paperclip size={14} />
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
                    height: rowH,
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
                    onOpen={() => openRow(vi.index)}
                    onAction={(kind) => {
                      if (kind === 'snooze') {
                        snoozeTomorrow(r.accountId, [r.id]);
                      } else if (kind === 'read') {
                        void dispatchAction({
                          accountId: r.accountId,
                          threadIds: [r.id],
                          action: { kind: 'read', on: r.unreadCount === 0 },
                        });
                      } else if (kind === 'star') {
                        void dispatchAction({
                          accountId: r.accountId,
                          threadIds: [r.id],
                          action: { kind: 'star', on: !r.isStarred },
                        });
                      } else {
                        void dispatchAction({
                          accountId: r.accountId,
                          threadIds: [r.id],
                          action: { kind },
                        });
                      }
                    }}
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
      {pickerHost && (
        <Popover
          open
          onOpenChange={(o) => {
            if (!o) setPickerHost(null);
          }}
          trigger={
            <button
              aria-hidden
              tabIndex={-1}
              style={{
                position: 'fixed',
                top: '28%',
                left: '50%',
                width: 1,
                height: 1,
                opacity: 0,
                pointerEvents: 'none',
              }}
            />
          }
        >
          <LabelPicker
            labels={hostLabels}
            selected={pickerHost.threadIds.map(
              (tid) => rows.find((x) => x.id === tid && x.accountId === pickerHost.accountId)?.labelIds ?? [],
            )}
            accountId={pickerHost.accountId}
            threadIds={pickerHost.threadIds}
            mode={pickerHost.kind}
            onDone={() => setPickerHost(null)}
          />
        </Popover>
      )}
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
