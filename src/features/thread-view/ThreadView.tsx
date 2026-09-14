import React, { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import type { MessageBody, ThreadDetail, Address, ActionKind } from '../../app/ipc/types';
import type { ComposeContext } from '../compose/replyContext';
import { useView } from '../../stores/viewStore';
import { useSelection } from '../../stores/selectionStore';
import { useSettings } from '../../stores/settingsStore';
import { MailFrame } from './MailFrame';
import { Avatar } from '../../ui/Avatar';
import { Chip } from '../../ui/Chip';
import { Spinner } from '../../ui/Spinner';
import { EmptyState } from '../../ui/EmptyState';
import { dispatchAction, dispatchGesture } from '../actions/dispatch';
import { useKeymap } from '../../keymap/engine';
import {
  openListPicker,
  runMailCommand,
  type ListPickerKind,
  type MailCommand,
} from '../thread-list/listCommands';
import { Star, Archive, Trash2, Clock, MoreHorizontal, Reply, Tag } from 'lucide-react';
import { IconButton } from '../../ui/IconButton';
import { Popover } from '../../ui/Popover';
import { Menu } from '../../ui/Menu';
import { SnoozeButton } from '../snooze/SnoozePopover';
import { LabelPicker } from '../actions/LabelPicker';
import { on } from '../../app/ipc/events';
import type { Label } from '../../app/ipc/types';
import { Button } from '../../ui/Button';
import { hasBlockingSurface } from '../../ui/overlayStack';
import { AttachmentStrip } from './AttachmentStrip';
import { decodeRfc2047 } from '../../lib/rfc2047';
import { labelKey, useLabels } from '../../stores/labelsStore';
import { cacheGet } from './bodyCache';
import { fetchBodiesNewestFirst, idsNewestFirst, pollBody, type BodyOutcome } from './bodyFetch';

/**
 * Thread metadata is paginated at 50 messages (P9.2): a 200-message
 * conversation opens on its newest page and reveals older messages through
 * "Show earlier", so neither the DOM nor the body cache starts at 200 rows.
 */
const MESSAGE_PAGE = 50;

/**
 * Auto-expansion stops this many messages from the end. A conversation with
 * fifty unread messages otherwise opened fifty bodies at once — "collapse old
 * messages" in the P9.2 brief — while the newest unread ones still open by
 * themselves the way a reader expects.
 */
const AUTO_EXPAND_LIMIT = 10;

function autoExpandFloor(count: number): number {
  return Math.max(0, count - AUTO_EXPAND_LIMIT);
}

/**
 * A body held in component state for the *active* thread. `settled` records how
 * polling ended, so a retryable failure becomes an explicit retry surface
 * instead of an endless skeleton or a fabricated message body.
 */
interface BodyEntry {
  body: MessageBody;
  settled?: BodyOutcome;
}

/** A message we could not load at all: no content, and never cached. */
function unloadedBody(messageId: string): MessageBody {
  return {
    messageId,
    state: 'loading',
    remoteImageCount: 0,
    trackerCount: 0,
    darkSafe: true,
    remoteImagesAllowed: false,
  };
}

export function ThreadView({
  onReply,
  obscured = false,
}: {
  onReply: (mode: string, context: ComposeContext) => void;
  /** A modal overlay (compose, palette, settings) covers the reader. */
  obscured?: boolean;
}) {
  const openThread = useView((s) => s.openThread);
  const [loaded, setLoaded] = useState<ThreadDetail | null>(null);
  // Only the active thread's bodies live here (P9.2); the shared, budgeted
  // cache in bodyCache.ts is what survives navigation.
  const [bodies, setBodies] = useState<Record<string, BodyEntry>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [focusMsg, setFocusMsg] = useState(0);
  // How many of the conversation's newest messages are rendered. Older ones are
  // revealed a page at a time (P9.2).
  const [visibleCount, setVisibleCount] = useState(MESSAGE_PAGE);
  const settings = useSettings((s) => s.settings);
  const markAsRead = settings.markAsRead;
  // Label names are indexed per account (P3.6); HeaderActions indexes the open
  // conversation's account as soon as its label list arrives.
  const labelNames = useLabels((s) => s.names);

  // Generation guards every async result for the open key (P3.2): a response
  // for an older click can never overwrite the current one. The abort
  // controller is the same guard for body polling (P9.2): navigating away
  // aborts *before* the next IPC instead of letting an old poll write into the
  // newly opened thread.
  const generationRef = useRef(0);
  const bodyAbortRef = useRef<AbortController | null>(null);
  /** Messages with a poll in flight, so re-rendering cannot double-fetch. */
  const pollingRef = useRef<Set<string>>(new Set());

  // Only the detail belonging to the current key is ever displayed, so the
  // previous thread's account/subject cannot leak into a new click.
  const detail =
    loaded && openThread && loaded.accountId === openThread.accountId && loaded.id === openThread.threadId
      ? loaded
      : null;

  const readTimer = useRef<number | null>(null);
  const onOpenMarkedRef = useRef<string | null>(null);
  const clearReadTimer = useCallback(() => {
    if (readTimer.current != null) {
      window.clearTimeout(readTimer.current);
      readTimer.current = null;
    }
  }, []);

  useEffect(() => {
    const generation = ++generationRef.current;
    // Captured for the cleanup: reading a ref during cleanup can observe a
    // different value than the one this effect actually operated on.
    const polling = pollingRef.current;
    onOpenMarkedRef.current = null;
    clearReadTimer();
    // Navigating away stops the previous thread's polling at the next check,
    // and no in-flight callback can write into the new thread's state (P9.2).
    bodyAbortRef.current?.abort();
    bodyAbortRef.current = null;
    pollingRef.current.clear();
    // Reset the displayed thread immediately: rows, bodies and expansion from
    // the previous key must not describe the newly clicked row.
    setLoaded(null);
    setBodies({});
    setExpanded({});
    setFocusMsg(0);
    setVisibleCount(MESSAGE_PAGE);
    if (!openThread) return;
    const controller = new AbortController();
    bodyAbortRef.current = controller;
    const { accountId, threadId } = openThread;
    let cancelled = false;
    void (async () => {
      const t0 = performance.now();
      const d = await api.thread_get(accountId, threadId).catch(() => null);
      if (cancelled || generationRef.current !== generation || !d) return;
      // The backend echoes the account it resolved; refuse a mismatched one.
      if (d.accountId !== accountId || d.id !== threadId) return;
      setLoaded(d);
      const exp: Record<string, boolean> = {};
      d.messages.forEach((m, i) => {
        exp[m.id] = i >= autoExpandFloor(d.messages.length) && (m.isUnread || i === d.messages.length - 1);
      });
      setExpanded(exp);
      const firstUnread = d.messages.findIndex((m) => m.isUnread);
      setFocusMsg(firstUnread < 0 ? 0 : firstUnread);
      const dt = performance.now() - t0;
      if (import.meta.env.DEV) console.debug(`[perf] thread-open ${dt.toFixed(1)}ms`);
    })();
    return () => {
      cancelled = true;
      controller.abort();
      if (bodyAbortRef.current === controller) bodyAbortRef.current = null;
      polling.clear();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openThread?.accountId, openThread?.threadId, clearReadTimer]);

  // Mark read only after the *same visible message* has stayed active for the
  // configured duration. Navigation, close, a covering modal and unmount all
  // clear the pending timer, so an abandoned hover never marks mail read.
  useEffect(() => {
    clearReadTimer();
    if (markAsRead === 'manual') return;
    if (obscured || !detail || !openThread) return;
    const threadKey = `${openThread.accountId}:${openThread.threadId}`;
    if (!detail.messages.some((m) => m.isUnread)) return;
    if (markAsRead === 'on-open') {
      if (onOpenMarkedRef.current === threadKey) return;
      onOpenMarkedRef.current = threadKey;
      void dispatchAction(
        {
          accountId: openThread.accountId,
          threadIds: [openThread.threadId],
          action: { kind: 'read', on: true },
        },
        { silent: true },
      );
      return clearReadTimer;
    }
    const visible = detail.messages[focusMsg] ?? detail.messages[0];
    if (!visible) return clearReadTimer;
    readTimer.current = window.setTimeout(() => {
      readTimer.current = null;
      const cur = useView.getState().openThread;
      const keyNow = cur ? `${cur.accountId}:${cur.threadId}` : null;
      if (keyNow !== threadKey) return; // navigated away
      void dispatchAction(
        {
          accountId: openThread.accountId,
          threadIds: [openThread.threadId],
          action: { kind: 'read', on: true },
        },
        { silent: true },
      );
    }, 2000);
    return clearReadTimer;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail, focusMsg, markAsRead, obscured, openThread?.accountId, openThread?.threadId, clearReadTimer]);

  /**
   * Fetch the expanded messages of the *active* thread, newest first, two at a
   * time (P9.2). Messages whose body is already cached are not requested again,
   * so a cached body paints immediately and never waits on a later CID or
   * provider round trip. The abort signal is the thread's, so navigation stops
   * the burst instead of letting it write into the next conversation.
   */
  useEffect(() => {
    if (!detail) return;
    const signal = bodyAbortRef.current?.signal;
    if (!signal || signal.aborted) return;
    const { accountId } = detail;
    const pending = idsNewestFirst(detail.messages.filter((m) => expanded[m.id])).filter(
      (id) => !pollingRef.current.has(id) && cacheGet(accountId, id) === undefined,
    );
    if (!pending.length) return;
    for (const id of pending) pollingRef.current.add(id);
    const settle = (messageId: string, outcome: BodyOutcome) => {
      pollingRef.current.delete(messageId);
      if (outcome === 'aborted') return;
      setBodies((prev) => {
        const current = prev[messageId];
        if (current) return { ...prev, [messageId]: { ...current, settled: outcome } };
        return { ...prev, [messageId]: { body: unloadedBody(messageId), settled: outcome } };
      });
    };
    void fetchBodiesNewestFirst(
      accountId,
      pending,
      (messageId, body) => {
        if (bodyAbortRef.current?.signal !== signal) return;
        setBodies((prev) => ({ ...prev, [messageId]: { body } }));
      },
      settle,
      signal,
    );
  }, [detail, expanded]);

  /**
   * Explicit retry for one message (P9.2). It reuses the thread's signal, so a
   * retry that outlives a navigation is aborted rather than delivered.
   */
  const retryBody = useCallback((messageId: string) => {
    const thread = useView.getState().openThread;
    const signal = bodyAbortRef.current?.signal;
    if (!thread || !signal || signal.aborted) return;
    if (pollingRef.current.has(messageId)) return;
    pollingRef.current.add(messageId);
    setBodies((prev) => {
      const next = { ...prev };
      delete next[messageId];
      return next;
    });
    void pollBody(
      thread.accountId,
      messageId,
      (body) => {
        if (bodyAbortRef.current?.signal !== signal) return;
        setBodies((prev) => ({ ...prev, [messageId]: { body } }));
      },
      signal,
    ).then((outcome) => {
      pollingRef.current.delete(messageId);
      if (outcome === 'aborted') return;
      setBodies((prev) => {
        const current = prev[messageId];
        if (current) return { ...prev, [messageId]: { ...current, settled: outcome } };
        return { ...prev, [messageId]: { body: unloadedBody(messageId), settled: outcome } };
      });
    });
  }, []);

  // Re-fetch the open thread when its rows change (actions, undo, sync).
  useEffect(() => {
    if (!openThread) return;
    const { accountId, threadId } = openThread;
    const generation = generationRef.current;
    let cancelled = false;
    let unsub = () => {};
    on<{ account_id: string; thread_ids: string[] }>('store:threads', (p) => {
      const payload = p as unknown as { account_id?: string; thread_ids?: string[] };
      const ids = payload.thread_ids ?? [];
      if (payload.account_id && payload.account_id !== accountId) return;
      if (ids.length > 0 && !ids.includes(threadId)) return;
      api
        .thread_get(accountId, threadId)
        .then((d) => {
          if (cancelled || generationRef.current !== generation) return;
          const cur = useView.getState().openThread;
          if (!cur || cur.accountId !== accountId || cur.threadId !== threadId) return;
          setLoaded(d);
        })
        .catch(() => {});
    })
      .then((u) => {
        if (cancelled) u();
        else unsub = u;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unsub();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openThread?.accountId, openThread?.threadId]);

  // Thread-scope keyboard: triage the open conversation without touching the
  // mouse. Configurable commands go through the keymap engine so remapping and
  // palette runs behave identically; Escape stays local because the engine
  // deliberately leaves it to the overlay/back-out handlers.
  const stale = detail == null;
  /**
   * The composer is told which message a reply belongs to (P5.4): the message
   * the reader has selected, or nothing so the composer picks the newest
   * non-draft message itself.
   */
  const replyContext: ComposeContext | null = detail
    ? {
        accountId: detail.accountId,
        threadId: detail.id,
        messageId: detail.messages[focusMsg]?.id,
      }
    : null;
  const stepMessage = (d: number) => {
    if (!detail) return;
    const n = Math.max(0, Math.min(detail.messages.length - 1, focusMsg + d));
    const m = detail.messages[n];
    if (m) setExpanded((ex) => ({ ...ex, [m.id]: true }));
    // Moving above the rendered page (p per the keymap) has to reveal that
    // message, otherwise the cursor would sit on a row that is not on screen.
    const start = Math.max(0, detail.messages.length - visibleCount);
    if (n < start) setVisibleCount((v) => Math.min(detail.messages.length, v + (start - n)));
    setFocusMsg(n);
  };
  const toggleMessage = () => {
    if (!detail) return;
    const m = detail.messages[focusMsg];
    if (m) setExpanded((ex) => ({ ...ex, [m.id]: !ex[m.id] }));
  };
  const pick = (what: ListPickerKind) => {
    // A selection belongs to the list; otherwise the picker targets the reader.
    if (useSelection.getState().selectedIds.size > 0 && openListPicker(what)) return;
    document.dispatchEvent(new CustomEvent('sift:thread-picker', { detail: { what } }));
  };
  const runThreadMail = (command: MailCommand, overrides?: { starOn?: boolean }) => {
    if (stale && useSelection.getState().selectedIds.size === 0) return;
    void runMailCommand(command, 'thread', overrides);
  };
  useKeymap(
    'thread',
    detail
      ? {
          back: () => {
            const v = useView.getState();
            if (v.paneLayout === 'off' && v.openThread != null) v.setOpenThread(null);
            else useSelection.getState().clearKeepFocus();
          },
          archive: () => runThreadMail('archive'),
          trash: () => runThreadMail('trash'),
          spam: () => runThreadMail('spam'),
          star: () => runThreadMail('star', { starOn: !detail.messages.some((m) => m.isStarred) }),
          markUnread: () => runThreadMail('markUnread'),
          markRead: () => runThreadMail('markRead'),
          snooze: () => pick('snooze'),
          label: () => pick('label'),
          move: () => pick('move'),
          reply: () => {
            if (!replyContext) return;
            onReply('reply', replyContext);
          },
          replyAll: () => {
            if (!replyContext) return;
            onReply('reply_all', replyContext);
          },
          forward: () => {
            if (!replyContext) return;
            onReply('forward', replyContext);
          },
          nextMsg: () => {
            if (stale) return;
            stepMessage(1);
          },
          prevMsg: () => {
            if (stale) return;
            stepMessage(-1);
          },
          toggleMsg: () => {
            if (stale) return;
            toggleMessage();
          },
          archiveNext: () => {
            document.dispatchEvent(new CustomEvent('sift:archive-nav', { detail: { dir: 1 as const } }));
            runThreadMail('archive');
          },
          archivePrev: () => {
            document.dispatchEvent(new CustomEvent('sift:archive-nav', { detail: { dir: -1 as const } }));
            runThreadMail('archive');
          },
        }
      : {},
  );

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      // An open menu/popover/dialog owns this Escape (P9.5); without this the
      // key would clear the reader selection *and* dismiss that surface.
      if (hasBlockingSurface()) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      const v = useView.getState();
      // Full-width reader: App backs out to the list.
      if (v.paneLayout === 'off' && v.openThread != null) return;
      useSelection.getState().clearKeepFocus();
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, []);

  const selected = useSelection((s) => s.selectedIds);
  const windowStart = detail ? Math.max(0, detail.messages.length - visibleCount) : 0;
  const visibleMessages = detail ? detail.messages.slice(windowStart) : [];
  const olderCount = detail ? windowStart : 0;
  if (selected.size > 1) {
    return (
      <div
        style={{
          flex: 1,
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 12,
        }}
      >
        <div style={{ fontSize: 14 }}>{selected.size} conversations selected</div>
        <BulkBar />
      </div>
    );
  }

  if (!openThread) {
    return (
      <div style={{ flex: 1 }}>
        <EmptyState line="Select a conversation" sub="j/k to move · Enter to open" />
      </div>
    );
  }
  if (!detail) {
    return (
      <div style={{ flex: 1, padding: 16 }}>
        <Spinner />
      </div>
    );
  }

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0, minHeight: 0 }}>
      <div
        style={{
          position: 'sticky',
          top: 0,
          background: 'var(--bg-pane)',
          borderBottom: '1px solid var(--border)',
          padding: '12px 20px 8px',
          zIndex: 2,
          minWidth: 0,
          overflow: 'hidden',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'flex-start', gap: 8, minWidth: 0 }}>
          <h1
            style={{
              fontSize: 20,
              fontWeight: 600,
              letterSpacing: '-0.01em',
              flex: 1,
              minWidth: 0,
              margin: 0,
              overflow: 'hidden',
              display: '-webkit-box',
              WebkitLineClamp: 3,
              WebkitBoxOrient: 'vertical',
              overflowWrap: 'anywhere',
              lineHeight: 1.25,
            }}
          >
            {decodeRfc2047(detail.subject) || '(No subject)'}
          </h1>
          <div style={{ flexShrink: 0 }}>
            <HeaderActions detail={detail} onReply={(mode) => onReply(mode, replyContext!)} />
          </div>
        </div>
        <div style={{ display: 'flex', gap: 6, marginTop: 6 }}>
          {detail.labelIds
            .filter((l) => !['INBOX', 'UNREAD'].includes(l))
            .map((l) => (
              <Chip key={l} label={labelNames[labelKey(detail.accountId, l)] ?? l} />
            ))}
        </div>
      </div>
      <div style={{ flex: 1, overflowY: 'auto', padding: '12px 20px 40px' }}>
        {/* Older messages are collapsed behind one explicit page control
            (P9.2): opening a 200-message conversation must not create 200
            message rows, bodies or fetches. */}
        {olderCount > 0 && (
          <div style={{ display: 'flex', justifyContent: 'center', marginBottom: 12 }}>
            <button className="sift-chip-btn" onClick={() => setVisibleCount((v) => v + MESSAGE_PAGE)}>
              Show earlier ({olderCount})
            </button>
          </div>
        )}
        {visibleMessages.map((m, offset) => {
          const i = windowStart + offset;
          const open = !!expanded[m.id];
          const entry = bodies[m.id];
          const body = entry?.body ?? cacheGet(detail.accountId, m.id);
          return (
            <div
              key={m.id}
              style={{
                border: '1px solid var(--border)',
                borderRadius: 'var(--r-lg)',
                marginBottom: 12,
                background: 'var(--n0)',
                overflow: 'hidden',
              }}
            >
              <button
                onClick={() => setExpanded((e) => ({ ...e, [m.id]: !e[m.id] }))}
                aria-expanded={open}
                data-testid={`msg-${m.id}`}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  width: '100%',
                  height: 40,
                  padding: '0 12px',
                  background: 'none',
                  border: 'none',
                  cursor: 'pointer',
                  textAlign: 'left',
                }}
              >
                <Avatar email={m.from.e} name={m.from.n} size={28} />
                <span
                  style={{
                    fontWeight: m.isUnread ? 600 : 400,
                    fontSize: 13,
                    flex: 1,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {m.from.n ?? m.from.e}
                </span>
                {!open && (
                  <span
                    style={{
                      color: 'var(--fg-3)',
                      fontSize: 13,
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                      maxWidth: 300,
                    }}
                  >
                    {m.snippet}
                  </span>
                )}
                <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-3)' }}>
                  {new Date(m.internalDate).toLocaleString()}
                </span>
                <span style={{ color: 'var(--fg-3)' }}>{open ? '▾' : '▸'}</span>
              </button>
              {open && (
                <div style={{ padding: '4px 16px 16px', borderTop: '1px solid var(--border)' }}>
                  <MessageMetaBar accountId={detail.accountId} m={m} />
                  {m.hasAttachments && (
                    <AttachmentStrip
                      accountId={detail.accountId}
                      messageId={m.id}
                      attachments={m.attachments}
                    />
                  )}
                  {/* A body is only ever rendered as content in the terminal
                      `ready` state (P9.2): a retryable failure shows an
                      explicit retry surface, never provider text dressed up as
                      the message. */}
                  {body?.state === 'ready' ? (
                    body.html ? (
                      <MailFrame
                        messageId={m.id}
                        html={body.html}
                        allowed={body.remoteImagesAllowed}
                        darkSafe={body.darkSafe}
                      />
                    ) : (
                      <pre
                        style={{
                          whiteSpace: 'pre-wrap',
                          fontFamily: 'var(--font-ui)',
                          fontSize: 14,
                          color: 'var(--fg)',
                        }}
                      >
                        {body.text || 'No content'}
                      </pre>
                    )
                  ) : entry?.settled ? (
                    <BodyError message={body?.text} onRetry={() => retryBody(m.id)} />
                  ) : (
                    <BodySkeleton />
                  )}
                  <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                    <button
                      onClick={() =>
                        onReply('reply', {
                          accountId: detail.accountId,
                          threadId: detail.id,
                          messageId: m.id,
                        })
                      }
                      style={{
                        background: 'none',
                        border: 'none',
                        cursor: 'pointer',
                        color: 'var(--fg-2)',
                        display: 'flex',
                        gap: 4,
                        alignItems: 'center',
                        fontSize: 12,
                      }}
                    >
                      <Reply size={14} /> Reply
                    </button>
                    <button
                      onClick={() => setFocusMsg(i)}
                      style={{
                        background: 'none',
                        border: 'none',
                        cursor: 'pointer',
                        color: 'var(--fg-3)',
                        fontSize: 12,
                      }}
                    >
                      ⋯ ({focusMsg === i ? 'focused' : 'focus'})
                    </button>
                  </div>
                </div>
              )}
            </div>
          );
        })}
        <div
          style={{
            border: '1px dashed var(--border-strong)',
            borderRadius: 'var(--r-lg)',
            padding: 12,
            display: 'flex',
            gap: 8,
          }}
        >
          <button
            onClick={() => replyContext && onReply('reply', replyContext)}
            style={{
              fontSize: 13,
              background: 'var(--n2)',
              border: '1px solid var(--border)',
              borderRadius: 6,
              padding: '6px 12px',
              cursor: 'pointer',
            }}
          >
            Reply ▾
          </button>
          <button
            onClick={() => replyContext && onReply('forward', replyContext)}
            style={{
              fontSize: 13,
              background: 'none',
              border: '1px solid var(--border)',
              borderRadius: 6,
              padding: '6px 12px',
              cursor: 'pointer',
            }}
          >
            Forward
          </button>
        </div>
      </div>
    </div>
  );
}

function HeaderActions({ detail, onReply }: { detail: ThreadDetail; onReply: (mode: string) => void }) {
  const { accountId, id: threadId } = detail;
  const starred = detail.messages.some((m) => m.isStarred);
  const unread = detail.messages.some((m) => m.isUnread);
  const [labels, setLabels] = useState<Label[]>([]);
  const [labelOpen, setLabelOpen] = useState(false);
  const [snoozeOpen, setSnoozeOpen] = useState(false);
  const [pickerMode, setPickerMode] = useState<'label' | 'move'>('label');
  useEffect(() => {
    api
      .labels_list(accountId)
      .then((ls) => {
        setLabels(ls);
        useLabels.getState().apply(accountId, ls);
      })
      .catch(() => {});
  }, [accountId]);
  // Bridge for thread-scope keyboard (h/l/v) handled in ThreadView.
  useEffect(() => {
    const h = (e: Event) => {
      const what = (e as CustomEvent).detail?.what as 'snooze' | 'label' | 'move' | undefined;
      if (what === 'snooze') setSnoozeOpen(true);
      else if (what === 'label' || what === 'move') {
        setPickerMode(what);
        setLabelOpen(true);
      }
    };
    document.addEventListener('sift:thread-picker', h as EventListener);
    return () => document.removeEventListener('sift:thread-picker', h as EventListener);
  }, []);
  const copyLink = () => {
    try {
      void navigator.clipboard.writeText(`https://mail.google.com/mail/u/0/#all/${threadId}`);
    } catch {
      /* clipboard unavailable */
    }
  };
  return (
    <div style={{ display: 'flex', gap: 4 }}>
      <IconButton
        tip={starred ? 'Unstar (s)' : 'Star (s)'}
        onClick={() =>
          void dispatchAction({
            accountId,
            threadIds: [threadId],
            action: { kind: 'star', on: !starred },
          })
        }
      >
        <Star
          size={16}
          fill={starred ? 'var(--star)' : 'none'}
          color={starred ? 'var(--star)' : 'currentColor'}
        />
      </IconButton>
      <IconButton
        tip="Archive (e)"
        onClick={() => void dispatchAction({ accountId, threadIds: [threadId], action: { kind: 'archive' } })}
      >
        <Archive size={16} />
      </IconButton>
      <IconButton
        tip="Trash (#)"
        onClick={() => void dispatchAction({ accountId, threadIds: [threadId], action: { kind: 'trash' } })}
      >
        <Trash2 size={16} />
      </IconButton>
      <SnoozeButton
        accountId={accountId}
        threadIds={[threadId]}
        open={snoozeOpen}
        onOpenChange={setSnoozeOpen}
        trigger={
          <button style={iconBtn} title="Snooze (h)" aria-label="Snooze" data-testid="thread-snooze">
            <Clock size={16} />
          </button>
        }
      />
      <Popover
        open={labelOpen}
        onOpenChange={setLabelOpen}
        trigger={
          <button style={iconBtn} title="Label (l) / Move (v)" aria-label="Label or move">
            <Tag size={16} />
          </button>
        }
      >
        <LabelPicker
          labels={labels}
          selected={detail.messages.map((m) => m.labelIds)}
          accountId={accountId}
          threadIds={[threadId]}
          mode={pickerMode}
          onDone={() => setLabelOpen(false)}
        />
      </Popover>
      <Menu
        trigger={
          <button style={iconBtn} title="More actions" aria-label="More actions">
            <MoreHorizontal size={16} />
          </button>
        }
        items={[
          { label: 'Reply', hint: 'r', action: () => onReply('reply') },
          { label: 'Reply all', hint: 'a', action: () => onReply('reply_all') },
          { label: 'Forward', hint: 'f', action: () => onReply('forward') },
          {
            label: unread ? 'Mark as read' : 'Mark as unread',
            hint: '⇧i',
            action: () =>
              void dispatchAction({
                accountId,
                threadIds: [threadId],
                action: { kind: 'read', on: !unread },
              }),
          },
          { label: 'Copy Gmail link', action: copyLink },
        ]}
      />
    </div>
  );
}

const iconBtn: React.CSSProperties = {
  width: 30,
  height: 30,
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  background: 'none',
  border: 'none',
  borderRadius: 6,
  cursor: 'pointer',
  color: 'var(--fg-2)',
};

function BulkBar() {
  const selected = useSelection((s) => s.selectedIds);
  const clear = useSelection((s) => s.clearSelection);
  const act = async (action: ActionKind) => {
    // One gesture, account-qualified targets: a mixed-account selection undoes
    // as the single action the user performed (P6.3).
    const targets = [...selected].flatMap((k) => {
      const i = k.indexOf(':');
      const accountId = k.slice(0, i);
      const threadId = k.slice(i + 1);
      return accountId && threadId ? [{ accountId, threadId }] : [];
    });
    await dispatchGesture(targets, action);
    clear();
  };
  return (
    <div style={{ display: 'flex', gap: 8 }}>
      <button onClick={() => void act({ kind: 'archive' })} className="sift-chip-btn">
        Archive
      </button>
      <button onClick={() => void act({ kind: 'trash' })} className="sift-chip-btn">
        Trash
      </button>
      <button
        onClick={clear}
        style={{
          padding: '6px 12px',
          borderRadius: 6,
          border: 'none',
          background: 'none',
          cursor: 'pointer',
          color: 'var(--fg-3)',
        }}
      >
        Clear
      </button>
    </div>
  );
}

function MessageMetaBar({ accountId, m }: { accountId: string; m: ThreadDetail['messages'][number] }) {
  const [open, setOpen] = useState(false);
  // Show the exact address in the summary (not "me"), so it is obvious which
  // identity a message was sent to when several accounts are in play.
  const short = (a: Address) => a.e;
  const full = (a: Address) => (a.n?.trim() ? `${a.n.trim()} <${a.e}>` : a.e);
  const summarize = (list: string[]) =>
    list.length <= 2 ? list.join(', ') : `${list.slice(0, 2).join(', ')} +${list.length - 2}`;

  const to = m.to.length ? m.to.map(short) : ['me'];
  const cc = m.cc.map(short);
  const rows: [string, string][] = [['From', full(m.from) || m.from.e]];
  rows.push(['To', m.to.map(full).join(', ') || 'me']);
  if (m.cc.length) rows.push(['Cc', m.cc.map(full).join(', ')]);
  if (m.bcc.length) rows.push(['Bcc', m.bcc.map(full).join(', ')]);
  if (m.replyTo) rows.push(['Reply-to', m.replyTo]);
  rows.push(['Date', new Date(m.internalDate).toLocaleString()]);

  return (
    <div style={{ margin: '8px 0', fontSize: 12, color: 'var(--fg-3)' }}>
      <div style={{ display: 'flex', gap: 6, alignItems: 'center', flexWrap: 'wrap' }}>
        <span>to {summarize(to)}</span>
        {cc.length > 0 && <span>· cc {summarize(cc)}</span>}
        {m.listUnsubscribe && <UnsubPill accountId={accountId} messageId={m.id} />}
        <button
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          style={{
            marginLeft: 'auto',
            background: 'none',
            border: 'none',
            color: 'var(--fg-3)',
            cursor: 'pointer',
            fontSize: 11.5,
            padding: 0,
          }}
        >
          {open ? 'Hide details' : 'Details'}
        </button>
      </div>
      {open && (
        <div
          style={{
            marginTop: 8,
            display: 'grid',
            gridTemplateColumns: 'auto minmax(0, 1fr)',
            gap: '3px 12px',
            maxWidth: 640,
            lineHeight: 1.5,
          }}
        >
          {rows.map(([k, v]) => (
            <Fragment key={k}>
              <span style={{ color: 'var(--fg-3)' }}>{k}</span>
              <span style={{ color: 'var(--fg-2)', overflowWrap: 'anywhere' }}>{v}</span>
            </Fragment>
          ))}
        </div>
      )}
    </div>
  );
}

function UnsubPill({ accountId, messageId }: { accountId: string; messageId: string }) {
  return (
    <button
      onClick={() => void api.unsubscribe(accountId, messageId)}
      style={{
        marginLeft: 8,
        fontSize: 11.5,
        border: '1px solid var(--border)',
        borderRadius: 999,
        padding: '1px 8px',
        background: 'var(--n1)',
        cursor: 'pointer',
        color: 'var(--fg-2)',
      }}
    >
      Unsubscribe
    </button>
  );
}

/**
 * Explicit retry surface for a message body (P9.2). The provider's own wording
 * may appear *here*, labelled as a failure; it is never rendered as the body.
 */
function BodyError({ message, onRetry }: { message?: string; onRetry: () => void }) {
  return (
    <div
      role="alert"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        padding: '8px 0',
        fontSize: 13,
        color: 'var(--fg-2)',
      }}
    >
      <span style={{ flex: 1, minWidth: 0 }}>
        Couldn’t load this message.
        {message ? (
          <span
            style={{
              display: 'block',
              marginTop: 2,
              color: 'var(--fg-3)',
              fontSize: 12,
              overflowWrap: 'anywhere',
            }}
          >
            {message}
          </span>
        ) : null}
      </span>
      <Button size="sm" variant="ghost" onClick={onRetry}>
        Retry
      </Button>
    </div>
  );
}

function BodySkeleton() {
  const [show, setShow] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setShow(true), 120);
    return () => clearTimeout(t);
  }, []);
  const ref = useRef(0);
  void ref;
  const cb = useCallback(() => {}, []);
  void cb;
  if (!show) return null;
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6, padding: '8px 0' }}>
      <div style={{ height: 12, background: 'var(--n2)', borderRadius: 4 }} />
      <div style={{ height: 12, background: 'var(--n2)', borderRadius: 4, width: '80%' }} />
      <div style={{ height: 12, background: 'var(--n2)', borderRadius: 4, width: '60%' }} />
    </div>
  );
}
