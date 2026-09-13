import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useThreadsWindow, type ThreadsWindowResult } from './useThreadsWindow';
import { ThreadRowView, rowDomId } from './ThreadRow';
import { rowHeightForDensity, densityScrollTop } from './rowHeight';
import { MAX_WINDOW, rowKey } from './threadWindow';
import { resolveCommandTargets, runMailCommand, setListContext, type ListPickerKind } from './listCommands';
import { useView } from '../../stores/viewStore';
import { useSelection } from '../../stores/selectionStore';
import { useAccounts } from '../../stores/accountsStore';
import { useLabels } from '../../stores/labelsStore';
import { useSettings } from '../../stores/settingsStore';
import { viewTitle } from '../../app/routes';
import { Spinner } from '../../ui/Spinner';
import { EmptyState } from '../../ui/EmptyState';
import { Skeleton } from '../../ui/Skeleton';
import { SyncPanel, SyncInlineBar, useSyncProgress } from '../sync/SyncPanel';
import { useScopedConnectivity } from '../sync/ConnectivityStrip';
import { relativeTime } from '../../lib/dates';
import { SearchInput } from '../search/SearchInput';
import { startServerSearch, useSearch, type SearchOrigin } from '../../stores/searchStore';
import { dispatchAction, undoLast } from '../actions/dispatch';
import { api } from '../../app/ipc/commands';
import { Popover } from '../../ui/Popover';
import { LabelPicker } from '../actions/LabelPicker';
import { toast } from 'sonner';
import type { Label } from '../../app/ipc/types';
import { useKeymap } from '../../keymap/engine';
import { Paperclip, Sun } from 'lucide-react';

export function ThreadList({ onCompose }: { onCompose: () => void }) {
  void onCompose;
  const view = useView((s) => s.view);
  const scope = useView((s) => s.accountScope);
  const setOpenThread = useView((s) => s.setOpenThread);
  const openThread = useView((s) => s.openThread);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [hasAtt, setHasAtt] = useState(false);
  const [pickerHost, setPickerHost] = useState<null | {
    kind: 'label' | 'move';
    accountId: string;
    threadIds: string[];
  }>(null);
  const [hostLabels, setHostLabels] = useState<Label[]>([]);
  const [labelName, setLabelName] = useState<string | null>(null);
  const [announce, setAnnounce] = useState<string | null>(null);

  // One visible search state (P7.2): a completed Gmail search replaces the
  // downloaded window instead of layering the two result sets. The window is
  // paused while the Gmail response owns the list.
  const search = useSearch();
  const gmailSearchActive =
    view.kind === 'search' && search.scope === 'server' && search.query.trim() === view.q;

  const {
    rows: windowRows,
    nextCursor: windowCursor,
    initialLoading: windowLoading,
    loadingMore,
    error: windowError,
    refreshRevision,
    queryGeneration,
    loadMore,
    reload,
  }: ThreadsWindowResult = useThreadsWindow(view, {
    unreadOnly,
    hasAttachment: hasAtt,
    paused: gmailSearchActive,
  });

  const rows = gmailSearchActive ? search.results : windowRows;
  const nextCursor = gmailSearchActive ? undefined : windowCursor;
  const initialLoading = gmailSearchActive ? search.loading : windowLoading;
  const error = gmailSearchActive ? search.error : windowError;
  const retry = gmailSearchActive ? startServerSearch : reload;

  const focusedKey = useSelection((s) => s.focusedKey);
  const setFocus = useSelection((s) => s.setFocus);
  const selectedIds = useSelection((s) => s.selectedIds);
  const accounts = useAccounts((s) => s.accounts);
  const included = useAccounts((s) => s.included);
  const density = useSettings((s) => s.settings.density);
  const rowH = rowHeightForDensity(density);
  const parentRef = useRef<HTMLDivElement>(null);
  const searchPending = view.kind === 'search' && search.loading;

  const keys = useMemo(() => rows.map(rowKey), [rows]);
  const focusedIndex = useMemo(() => {
    const i = focusedKey ? keys.indexOf(focusedKey) : -1;
    return i < 0 ? 0 : i;
  }, [keys, focusedKey]);

  const colorOf = useCallback(
    (accountId: string) => accounts.find((a) => a.id === accountId)?.color ?? 'blue',
    [accounts],
  );
  const labelOf = useCallback(
    (accountId: string) => accounts.find((a) => a.id === accountId)?.email ?? undefined,
    [accounts],
  );
  const includedAccounts = accounts.filter((a) => included[a.id] !== false);
  // One account in view → the marker carries no information.
  const showStripe = scope === 'all' && includedAccounts.length > 1;
  const scopedIds = scope === 'all' ? accounts.map((a) => a.id) : [scope];
  // Row chips and the reader show label names, indexed per account (P3.6).
  // The index is fetched once per account and refreshed when labels change.
  const scopedKey = scopedIds.join(',');
  useEffect(() => {
    useLabels.getState().ensure(scopedIds);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopedKey]);
  const progress = useSyncProgress(scopedIds);
  // The empty list reports the real last successful sync (or the cached/offline
  // state) instead of always claiming "just now" (P3.6/P4.6).
  const scoped = useScopedConnectivity(scopedIds);
  const emptySub = scoped.rows.length
    ? 'Offline — showing cached mail'
    : scoped.lastOkAt
      ? `Last synced ${relativeTime(scoped.lastOkAt)}`
      : 'Not synced yet';
  const retrySync = useCallback(() => {
    const ids = scope === 'all' ? accounts.filter((a) => a.sync_state === 'error').map((a) => a.id) : [scope];
    for (const id of ids) void api.sync_now(id).catch(() => {});
  }, [scope, accounts]);

  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowH,
    getItemKey: useCallback((index: number) => keys[index] ?? `row-${index}`, [keys]),
    overscan: 8,
  });

  const openRow = useCallback(
    (index: number) => {
      const r = rows[index];
      if (!r) return;
      // The opened row becomes the keyboard-active row (P3.2 #6).
      setFocus(rowKey(r));
      setOpenThread({ accountId: r.accountId, threadId: r.id });
    },
    [rows, setFocus, setOpenThread],
  );

  /**
   * Keyboard navigation owns its own scrolling: the focused row is brought
   * into view only when the user actually moved the focus. A refresh that
   * shifts the focused row's index or removes it must not touch the scroll
   * offset — restoring the visible anchor is the refresh path's job
   * (P3.4/P3.5), and this effect previously overrode it with a jump to the top.
   */
  const scrollFocusedIntoView = useCallback(
    (key: string | null | undefined) => {
      if (!key) return;
      const i = keys.indexOf(key);
      if (i < 0) return;
      virtual.scrollToIndex(i, { align: 'auto' });
    },
    [keys, virtual],
  );

  const moveCursor = useCallback(
    (d: number) => {
      useSelection.getState().move(d, keys);
      const nextKey = useSelection.getState().focusedKey;
      scrollFocusedIntoView(nextKey);
      // The reading pane follows the keyboard cursor (spec 12.2).
      if (nextKey && useView.getState().paneLayout !== 'off') {
        const r = rows.find((x) => rowKey(x) === nextKey);
        if (r) setOpenThread({ accountId: r.accountId, threadId: r.id });
      }
    },
    [keys, rows, setOpenThread, scrollFocusedIntoView],
  );

  const extendSelection = useCallback(
    (d: number) => {
      const current = useSelection.getState().focusedKey;
      const i = current ? keys.indexOf(current) : -1;
      const at = i < 0 ? (d > 0 ? -1 : 0) : i;
      const target = keys[Math.max(0, Math.min(keys.length - 1, at + d))];
      if (target) {
        useSelection.getState().extendTo(target, keys);
        scrollFocusedIntoView(target);
      }
    },
    [keys, scrollFocusedIntoView],
  );

  const snoozeTomorrow = useCallback((accountId: string, threadIds: string[]) => {
    const d = new Date();
    d.setDate(d.getDate() + 1);
    d.setHours(8, 0, 0, 0);
    void api.snooze_set(accountId, threadIds, d.getTime()).then(() => {
      toast('Snoozed until Tomorrow 08:00', {
        action: { label: 'Undo (z)', onClick: () => void undoLast() },
      });
    });
  }, []);

  const openPicker = useCallback(
    (kind: ListPickerKind) => {
      const targets = resolveCommandTargets('list');
      const first = targets[0];
      if (!first) return;
      if (kind === 'snooze') {
        snoozeTomorrow(first.accountId, first.threadIds);
        return;
      }
      setPickerHost({ kind, accountId: first.accountId, threadIds: first.threadIds });
    },
    [snoozeTomorrow],
  );

  // The mounted list owns command targets for the keymap engine and the
  // palette; publish the current window and clear it on unmount.
  useEffect(() => {
    return setListContext({ rows, openPicker });
  }, [rows, openPicker]);

  // Configurable list commands route through the keymap engine, so remapping a
  // binding (or running it from the palette) never touches this component.
  const listMap: Record<string, () => void> = {
    focusNext: () => moveCursor(1),
    focusPrev: () => moveCursor(-1),
    open: () => openRow(focusedIndex),
    toggleSelect: () => {
      const key = useSelection.getState().focusedKey;
      if (key) useSelection.getState().toggle(key);
    },
    extendDown: () => extendSelection(1),
    extendUp: () => extendSelection(-1),
    selectAll: () => {
      useSelection.getState().selectAll(keys);
      setAnnounce(`Selected ${keys.length} conversations`);
    },
    archive: () => void runMailCommand('archive', 'list'),
    trash: () => void runMailCommand('trash', 'list'),
    spam: () => void runMailCommand('spam', 'list'),
    star: () => void runMailCommand('star', 'list'),
    markUnread: () => void runMailCommand('markUnread', 'list'),
    markRead: () => void runMailCommand('markRead', 'list'),
    snooze: () => openPicker('snooze'),
    label: () => openPicker('label'),
    move: () => openPicker('move'),
  };
  useKeymap('list', searchPending ? {} : listMap);

  // Escape clears the selection. Modals own Escape first (App's capture
  // handler stops propagation), so this never fights a dialog.
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (useSelection.getState().selectedIds.size === 0) return;
      e.preventDefault();
      useSelection.getState().clearSelection();
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, []);

  // A new query identity (view, account scope or filter) starts at the top:
  // the previous window's anchor describes rows that are no longer loaded.
  // Only a refresh of the *same* query keeps the anchor (P3.4/P3.5).
  const anchorRef = useRef<{ key: string; offset: number } | null>(null);
  const pendingRestore = useRef<SearchOrigin | null>(null);
  useEffect(() => {
    anchorRef.current = null;
    const el = parentRef.current;
    if (el) el.scrollTop = 0;
    // A pending search restore only survives the view it belongs to.
    const target = pendingRestore.current;
    if (target && JSON.stringify(target.view) !== JSON.stringify(view)) pendingRestore.current = null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryGeneration]);

  const captureSearchOrigin = useCallback(
    (): SearchOrigin => ({
      view: useView.getState().view,
      accountScope: useView.getState().accountScope,
      anchorKey: anchorRef.current?.key ?? null,
      anchorOffset: anchorRef.current?.offset ?? 0,
      focusedKey: useSelection.getState().focusedKey,
    }),
    [],
  );

  /**
   * Leaving search puts the user back in the mailbox they searched from: the
   * view and account scope are restored immediately, and the anchor row, its
   * offset and the focused row are applied once that window is loaded again
   * (P7.2).
   */
  const restoreSearchOrigin = useCallback((origin: SearchOrigin) => {
    pendingRestore.current = origin;
    if (useView.getState().accountScope !== origin.accountScope) {
      useView.getState().setScope(origin.accountScope);
    }
    useView.getState().setView(origin.view);
    if (origin.focusedKey) useSelection.getState().setFocus(origin.focusedKey);
  }, []);

  // A window refresh replaces rows in place; keep the row that was at the top
  // of the viewport (and its offset) there instead of letting the list jump.
  useEffect(() => {
    const el = parentRef.current;
    const anchor = anchorRef.current;
    if (!el || !anchor) return;
    const index = rows.findIndex((r) => rowKey(r) === anchor.key);
    if (index < 0) return;
    el.scrollTop = index * rowH + anchor.offset;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshRevision]);

  // Record the top visible row after the restore above has run. Both the key
  // and the offset are stored, so the anchor survives a refresh that inserts or
  // removes rows above the viewport (P3.4/P3.5).
  useEffect(() => {
    const el = parentRef.current;
    const first = virtual.getVirtualItems()[0];
    if (!el || !first) return;
    const row = rows[first.index];
    if (row) anchorRef.current = { key: rowKey(row), offset: el.scrollTop - first.start };
  });

  // Apply a pending search restore as soon as its window is loaded. The anchor
  // row can sit further down the window than the first page — paging back to it
  // (bounded by the window cap) is what makes Escape return to the same
  // reading position instead of to the top.
  useEffect(() => {
    const el = parentRef.current;
    const target = pendingRestore.current;
    if (!el || !target) return;
    if (initialLoading) return;
    if (!target.anchorKey) {
      pendingRestore.current = null;
      return;
    }
    const index = rows.findIndex((r) => rowKey(r) === target.anchorKey);
    if (index < 0) {
      if (nextCursor && !loadingMore && rows.length < MAX_WINDOW) loadMore();
      else pendingRestore.current = null;
      return;
    }
    el.scrollTop = index * rowH + target.anchorOffset;
    anchorRef.current = { key: target.anchorKey, offset: target.anchorOffset };
    pendingRestore.current = null;
  }, [rows, initialLoading, nextCursor, loadingMore, loadMore, rowH]);

  // Density changes invalidate every measured row. Reset measurement, then put
  // the previously top visible row and its offset back where they were.
  const prevRowH = useRef(rowH);
  useEffect(() => {
    const prev = prevRowH.current;
    if (prev === rowH) return;
    prevRowH.current = rowH;
    const el = parentRef.current;
    if (!el) return;
    const first = virtual.getVirtualItems()[0];
    const index = first?.index ?? 0;
    const within = first ? el.scrollTop - first.start : 0;
    virtual.measure();
    const top = densityScrollTop(index, within, prev, rowH);
    requestAnimationFrame(() => {
      el.scrollTop = top;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rowH]);

  // Archive-and-go (]/[ from the thread view): archive happened there; advance here.
  useEffect(() => {
    const h = (e: Event) => {
      const dir = (e as CustomEvent).detail?.dir as 1 | -1;
      const current = useSelection.getState().focusedKey;
      const i = current ? keys.indexOf(current) : -1;
      const at = i < 0 ? 0 : i;
      const next = Math.max(0, Math.min(rows.length - 1, at + dir));
      const r = rows[next];
      if (!r) return;
      setFocus(rowKey(r));
      scrollFocusedIntoView(rowKey(r));
      if (useView.getState().paneLayout !== 'off') openRow(next);
    };
    document.addEventListener('sift:archive-nav', h as EventListener);
    return () => document.removeEventListener('sift:archive-nav', h as EventListener);
  }, [rows, keys, setFocus, openRow, scrollFocusedIntoView]);

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
    const id = view.labelId;
    const aids = scope === 'all' ? accounts.map((a) => a.id) : [scope];
    Promise.all(aids.map((a) => api.labels_list(a).catch(() => [] as Label[]))).then((all) => {
      const found = all.flat().find((l) => l.id === id);
      setLabelName(found ? (found.name.split('/').pop() ?? found.name) : id);
    });
  }, [view, scope, accounts]);

  // Auto-open the focused conversation when the pane is visible but nothing is
  // open yet (first load, view or account switch), like Apple Mail. Once a
  // thread is selected this never fires, so it cannot fight the user.
  useEffect(() => {
    if (useView.getState().paneLayout === 'off') return;
    if (useView.getState().openThread != null) return;
    if (!rows.length) return;
    const r = rows[Math.min(focusedIndex, rows.length - 1)];
    if (r) {
      setFocus(rowKey(r));
      setOpenThread({ accountId: r.accountId, threadId: r.id });
    }
  }, [rows, focusedIndex, setOpenThread, setFocus]);

  // If the open conversation leaves the current view (trashed, archived, moved
  // away, or removed by sync), move the reading pane to the closest remaining
  // one instead of leaving a stale message on screen.
  useEffect(() => {
    if (useView.getState().paneLayout === 'off') return;
    const open = useView.getState().openThread;
    if (open == null) return;
    if (initialLoading) return;
    if (rows.some((r) => r.accountId === open.accountId && r.id === open.threadId)) return;
    if (!rows.length) {
      setOpenThread(null);
      return;
    }
    const i = Math.min(focusedIndex, rows.length - 1);
    const r = rows[i];
    if (r) {
      setFocus(rowKey(r));
      setOpenThread({ accountId: r.accountId, threadId: r.id });
    }
  }, [rows, initialLoading, focusedIndex, setFocus, setOpenThread]);

  const title = useMemo(() => {
    if (view.kind === 'label') return labelName ?? view.labelId;
    if (view.kind === 'search') return `Results for “${view.q}”`;
    return viewTitle[view.kind] ?? 'Inbox';
  }, [view, labelName]);

  // `aria-activedescendant` may only point at a row that is actually rendered:
  // the list is virtualized, so the active row can be outside the window.
  //
  // The rendered window keeps its generous tail margin (paging walks and fast
  // scrolling need the rows below) but drops the rows parked far *above* the
  // viewport. Those rows are what the browser's own `scrollIntoViewIfNeeded`
  // targets when a pointer click (or assistive focus) lands on them, and that
  // scroll silently redefines the reading anchor the refresh restore depends on
  // — no anchor policy can undo a scroll the app never initiated (P3.4/P3.5).
  const virtualItems = virtual.getVirtualItems();
  const scrollOffset = virtual.scrollOffset ?? 0;
  const firstRenderedIndex = Math.max(0, Math.floor(scrollOffset / rowH) - 1);
  const renderedItems = virtualItems.filter((vi) => vi.index >= firstRenderedIndex);
  const focusedRowIndex = focusedKey ? keys.indexOf(focusedKey) : -1;
  const activeDescendant =
    focusedRowIndex >= 0 && renderedItems.some((vi) => vi.index === focusedRowIndex)
      ? rowDomId(rows[focusedRowIndex])
      : undefined;

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
        {selectedIds.size > 0 && (
          <span
            aria-live="polite"
            style={{ fontSize: 12, color: 'var(--fg-2)', flexShrink: 0, fontVariantNumeric: 'tabular-nums' }}
          >
            {selectedIds.size} selected
          </span>
        )}
        <span aria-live="polite" style={SR_ONLY_LIVE}>
          {announce ?? ''}
        </span>
        <SearchInput captureOrigin={captureSearchOrigin} restoreOrigin={restoreSearchOrigin} />
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
      {rows.length > 0 && <SyncInlineBar progress={progress} />}
      {error && rows.length > 0 && (
        <div
          role="alert"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            padding: '6px 12px',
            fontSize: 12,
            color: 'var(--fg-2)',
            borderBottom: '1px solid var(--border)',
          }}
        >
          <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>{error}</span>
          <button className="sift-chip-btn" onClick={retry}>
            Retry
          </button>
        </div>
      )}
      <div
        ref={parentRef}
        style={{ flex: 1, overflowY: 'auto', position: 'relative' }}
        role="listbox"
        // One tab stop for the whole list (P9.5): the scrollable region is
        // reachable from the keyboard and reports the active row through
        // aria-activedescendant instead of holding focus on each row.
        tabIndex={0}
        aria-label={title}
        aria-multiselectable
        aria-activedescendant={activeDescendant}
      >
        {rows.length === 0 ? (
          error ? (
            <QueryErrorState message={error} onRetry={retry} />
          ) : progress.active || progress.failed ? (
            <SyncPanel progress={progress} onRetry={retrySync} />
          ) : initialLoading ? (
            <DelayedSkeleton />
          ) : view.kind === 'inbox' ? (
            <div style={{ animation: 'sift-fade 400ms var(--ease-out)' }}>
              <style>{'@keyframes sift-fade { from { opacity: 0; } }'}</style>
              <EmptyState icon={<Sun size={24} />} line="You're all caught up." sub={emptySub} />
            </div>
          ) : (
            <EmptyState
              line="No conversations match."
              sub={view.kind === 'search' ? 'Search Gmail instead ↩' : undefined}
            />
          )
        ) : (
          <div style={{ height: virtual.getTotalSize(), position: 'relative' }}>
            {renderedItems.map((vi) => {
              const r = rows[vi.index];
              if (!r) return null;
              const key = rowKey(r);
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
                    accountLabel={labelOf(r.accountId)}
                    showStripe={showStripe}
                    onFocus={() => setFocus(key)}
                    onToggleSelect={(e) => {
                      if (e.shiftKey) useSelection.getState().extendTo(key, keys);
                      else useSelection.getState().toggle(key);
                    }}
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
        {!initialLoading && nextCursor ? <ScrollSentinel onVisible={loadMore} /> : null}
        {loadingMore && rows.length > 0 && (
          <div style={{ padding: 8, display: 'flex', justifyContent: 'center' }}>
            <Spinner size={12} />
          </div>
        )}
      </div>
      {openThread && <div style={{ display: 'none' }}>{openThread.accountId}:{openThread.threadId}</div>}
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

const SR_ONLY_LIVE: React.CSSProperties = {
  position: 'absolute',
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: 'hidden',
  clipPath: 'inset(50%)',
  whiteSpace: 'nowrap',
  border: 0,
};

/** Query failure is a retryable state — never the empty-inbox copy. */
export function QueryErrorState({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div style={{ padding: 24, textAlign: 'center' }}>
      <div style={{ fontSize: 14, fontWeight: 600, marginBottom: 4 }}>Couldn’t load conversations.</div>
      <div style={{ fontSize: 12, color: 'var(--fg-3)', marginBottom: 12 }}>{message}</div>
      <button className="sift-chip-btn" onClick={onRetry}>
        Retry
      </button>
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
