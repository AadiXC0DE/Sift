import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import type { MessageBody, ThreadDetail } from '../../app/ipc/types';
import { useView } from '../../stores/viewStore';
import { useSelection } from '../../stores/selectionStore';
import { useSettings } from '../../stores/settingsStore';
import { MailFrame } from './MailFrame';
import { Avatar } from '../../ui/Avatar';
import { Chip } from '../../ui/Chip';
import { Spinner } from '../../ui/Spinner';
import { EmptyState } from '../../ui/EmptyState';
import { dispatchAction } from '../actions/dispatch';
import { Star, Archive, Trash2, Clock, MoreHorizontal, Reply, Paperclip, Tag, Download } from 'lucide-react';
import { IconButton } from '../../ui/IconButton';
import { Popover } from '../../ui/Popover';
import { Menu } from '../../ui/Menu';
import { SnoozeButton } from '../snooze/SnoozePopover';
import { LabelPicker } from '../actions/LabelPicker';
import { on } from '../../app/ipc/events';
import type { Label } from '../../app/ipc/types';
import { Button } from '../../ui/Button';
import { decodeRfc2047 } from '../../lib/rfc2047';

const bodyCache = new Map<string, MessageBody>();
function cacheGet(id: string) {
  return bodyCache.get(id);
}
function cacheSet(id: string, b: MessageBody) {
  bodyCache.set(id, b);
  if (bodyCache.size > 50) {
    const first = bodyCache.keys().next().value;
    if (first) bodyCache.delete(first);
  }
}

async function pollBody(id: string, onUpdate: (b: MessageBody) => void) {
  let delay = 250;
  let last: MessageBody | undefined;
  for (let i = 0; i < 16; i++) {
    try {
      const b = await api.message_body(id);
      last = b;
      cacheSet(id, b);
      onUpdate(b);
      if (b.state === 'ready' || b.state === 'error') {
        return;
      }
    } catch {
      onUpdate({
        messageId: id,
        state: 'error',
        remoteImageCount: 0,
        trackerCount: 0,
        darkSafe: true,
        remoteImagesAllowed: false,
        text: last?.text,
      });
      return;
    }
    await new Promise((r) => setTimeout(r, delay));
    delay = Math.min(Math.round(delay * 1.35), 2000);
  }
  if (last && last.state === 'loading') {
    onUpdate({ ...last, state: last.text || last.html ? 'ready' : 'error' });
  }
}

export function ThreadView({ onReply }: { onReply: (mode: string, threadId: string) => void }) {
  const threadId = useView((s) => s.threadId);
  const scope = useView((s) => s.accountScope);
  const view = useView((s) => s.view);
  const [detail, setDetail] = useState<ThreadDetail | null>(null);
  const [bodies, setBodies] = useState<Record<string, MessageBody>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [focusMsg, setFocusMsg] = useState(0);
  const settings = useSettings((s) => s.settings);
  const markAsRead = settings.markAsRead;
  void scope;
  void view;

  // find account for thread: use current scope or first message's account after load
  const _accountId = useMemo(() => {
    if (scope !== 'all') return scope;
    return detail?.accountId ?? '';
  }, [scope, detail]);
  void _accountId;

  useEffect(() => {
    if (!threadId) {
      setDetail(null);
      return;
    }
    const aid = scope === 'all' ? undefined : scope;
    // resolve account: try scope, else search loaded detail
    const run = async () => {
      // When unified, we need accountId: look up via threads_query? Simplify: use detail?.accountId or first account.
      // For now, if scope is 'all', fetch via all accounts by trying thread_get on each is expensive;
      // the list row knows accountId - ThreadList should set it. Fallback: use stored last account.
      const account =
        aid ?? detail?.accountId ?? (window as unknown as { __lastAccount?: string }).__lastAccount ?? '';
      if (!account) return;
      const t0 = performance.now();
      const d = await api.thread_get(account, threadId).catch(() => null);
      if (!d) return;
      setDetail(d);
      (window as unknown as { __lastAccount?: string }).__lastAccount = d.accountId;
      const exp: Record<string, boolean> = {};
      d.messages.forEach((m, i) => {
        exp[m.id] = m.isUnread || i === d.messages.length - 1;
      });
      setExpanded(exp);
      setFocusMsg(d.messages.findIndex((m) => m.isUnread));
      // bodies fetched in the expanded-ids effect below
      // mark as read
      if (markAsRead === 'on-open') {
        const unread = d.messages.some((m) => m.isUnread);
        if (unread)
          void dispatchAction(
            { accountId: d.accountId, threadIds: [d.id], action: { kind: 'read', on: true } },
            { silent: true },
          );
      } else if (markAsRead === 'after-2s') {
        setTimeout(() => {
          void dispatchAction(
            { accountId: d.accountId, threadIds: [d.id], action: { kind: 'read', on: true } },
            { silent: true },
          );
        }, 2000);
      }
      const dt = performance.now() - t0;
      if (import.meta.env.DEV) console.debug(`[perf] thread-open ${dt.toFixed(1)}ms`);
    };
    run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId]);

  useEffect(() => {
    if (!detail) return;
    let cancelled = false;
    const ids = detail.messages.filter((m) => expanded[m.id]).map((m) => m.id);
    void (async () => {
      for (const id of ids) {
        if (cancelled) return;
        const cached = cacheGet(id);
        if (cached?.state === 'ready' || cached?.state === 'error') {
          setBodies((p) => (p[id] ? p : { ...p, [id]: cached }));
          continue;
        }
        await pollBody(id, (b) => {
          if (!cancelled) setBodies((p) => ({ ...p, [id]: b }));
        });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [detail, expanded]);

  // Re-fetch the open thread when its rows change (actions, undo, sync).
  useEffect(() => {
    if (!threadId) return;
    let unsub = () => {};
    on<{ account_id: string; thread_ids: string[] }>('store:threads', (p) => {
      const ids = (p as unknown as { thread_ids: string[] }).thread_ids ?? [];
      if (!ids.includes(threadId) && ids.length > 0) return;
      const acc = (window as unknown as { __lastAccount?: string }).__lastAccount;
      if (acc)
        api
          .thread_get(acc, threadId)
          .then(setDetail)
          .catch(() => {});
    })
      .then((u) => {
        unsub = u;
      })
      .catch(() => {});
    return () => unsub();
  }, [threadId]);

  // Thread-scope keyboard: triage the open conversation without touching the mouse.
  useEffect(() => {
    if (!detail) return;
    const ids = { accountId: detail.accountId, threadIds: [detail.id] };
    const h = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const k = e.key;
      // Leave full-width reader back to the list (palette/compose own their Esc).
      if (k === 'Escape' || k === 'u') {
        const w = window as unknown as { __paletteOpen?: boolean; __composeOpen?: boolean };
        if (
          useView.getState().paneLayout === 'off' &&
          !w.__paletteOpen &&
          !w.__composeOpen &&
          (k === 'u' || useView.getState().threadId != null)
        ) {
          e.preventDefault();
          useView.getState().setThread(null);
          return;
        }
        if (k === 'Escape') return;
        useSelection.getState().clearKeepFocus();
        return;
      }
      // Ignore action keys while the detail is stale (cursor moved on).
      if (detail.id !== useView.getState().threadId) return;
      const starred = detail.messages.some((m) => m.isStarred);
      const focusStep = (d: number) => {
        setFocusMsg((v) => {
          const n = Math.max(0, Math.min(detail.messages.length - 1, v + d));
          const m = detail.messages[n];
          if (m) setExpanded((ex) => ({ ...ex, [m.id]: true }));
          return n;
        });
      };
      const pick = (what: 'snooze' | 'label' | 'move') =>
        document.dispatchEvent(new CustomEvent('sift:thread-picker', { detail: { what } }));
      if (k === 'e') void dispatchAction({ ...ids, action: { kind: 'archive' } });
      else if (k === '#' || k === 'Backspace') {
        e.preventDefault();
        void dispatchAction({ ...ids, action: { kind: 'trash' } });
      } else if (k === '!') void dispatchAction({ ...ids, action: { kind: 'spam' } });
      else if (k === 's') void dispatchAction({ ...ids, action: { kind: 'star', on: !starred } });
      else if (k === 'U') void dispatchAction({ ...ids, action: { kind: 'read', on: false } });
      else if (k === 'I') void dispatchAction({ ...ids, action: { kind: 'read', on: true } });
      else if (k === 'h') pick('snooze');
      else if (k === 'l') pick('label');
      else if (k === 'v') pick('move');
      else if (k === 'r') onReply('reply', detail.id);
      else if (k === 'a') onReply('reply_all', detail.id);
      else if (k === 'f') onReply('forward', detail.id);
      else if (k === 'n') focusStep(1);
      else if (k === 'p') focusStep(-1);
      else if (k === 'o') {
        const m = detail.messages[focusMsg];
        if (m) setExpanded((ex) => ({ ...ex, [m.id]: !ex[m.id] }));
      } else if (k === ']') {
        document.dispatchEvent(new CustomEvent('sift:archive-nav', { detail: { dir: 1 as const } }));
        void dispatchAction({ ...ids, action: { kind: 'archive' } });
      } else if (k === '[') {
        document.dispatchEvent(new CustomEvent('sift:archive-nav', { detail: { dir: -1 as const } }));
        void dispatchAction({ ...ids, action: { kind: 'archive' } });
      }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [detail, focusMsg, onReply]);

  const selected = useSelection((s) => s.selectedIds);
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

  if (!threadId) {
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
            <HeaderActions detail={detail} onReply={(mode) => onReply(mode, detail.id)} />
          </div>
        </div>
        <div style={{ display: 'flex', gap: 6, marginTop: 6 }}>
          {detail.labelIds
            .filter((l) => !['INBOX', 'UNREAD'].includes(l))
            .map((l) => (
              <Chip key={l} label={l} />
            ))}
        </div>
      </div>
      <div style={{ flex: 1, overflowY: 'auto', padding: '12px 20px 40px' }}>
        {detail.messages.map((m, i) => {
          const open = !!expanded[m.id];
          const body = bodies[m.id] ?? cacheGet(m.id);
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
                  <div style={{ fontSize: 12, color: 'var(--fg-3)', margin: '8px 0' }}>
                    to {m.to.map((a) => a.e).join(', ') || 'me'}
                    {m.cc.length > 0 && (
                      <details style={{ display: 'inline', marginLeft: 8 }}>
                        <summary style={{ display: 'inline', cursor: 'pointer' }}>cc ▾</summary>{' '}
                        {m.cc.map((a) => a.e).join(', ')}
                      </details>
                    )}
                    {m.listUnsubscribe && <UnsubPill messageId={m.id} />}
                  </div>
                  {m.hasAttachments && <AttachmentStrip messageId={m.id} attachments={m.attachments} />}
                  {!body || body.state === 'loading' ? (
                    body?.text ? (
                      <pre
                        style={{
                          whiteSpace: 'pre-wrap',
                          fontFamily: 'var(--font-ui)',
                          fontSize: 14,
                          color: 'var(--fg)',
                        }}
                      >
                        {body.text}
                      </pre>
                    ) : (
                      <BodySkeleton />
                    )
                  ) : body.state === 'error' ? (
                    <div style={{ color: 'var(--danger)', fontSize: 13 }}>
                      {body.text || "Couldn't load this message."}{' '}
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => void pollBody(m.id, (b) => setBodies((p) => ({ ...p, [m.id]: b })))}
                      >
                        Retry
                      </Button>
                    </div>
                  ) : body.html ? (
                    <MailFrame
                      messageId={m.id}
                      html={body.html}
                      allowed={body.remoteImagesAllowed}
                      dark={false}
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
                  )}
                  <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                    <button
                      onClick={() => onReply('reply', detail.id)}
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
            onClick={() => onReply('reply', detail.id)}
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
            onClick={() => onReply('forward', detail.id)}
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
      .then(setLabels)
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
          <button style={iconBtn} title="Snooze (h)">
            <Clock size={16} />
          </button>
        }
      />
      <Popover
        open={labelOpen}
        onOpenChange={setLabelOpen}
        trigger={
          <button style={iconBtn} title="Label (l) / Move (v)">
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
          <button style={iconBtn} title="More actions">
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
  const act = async (kind: 'archive' | 'trash') => {
    // split per account
    const byAcc: Record<string, string[]> = {};
    for (const k of selected) {
      const [acc, ...rest] = k.split(':');
      const tid = rest.join(':');
      (byAcc[acc] ??= []).push(tid);
    }
    for (const [acc, tids] of Object.entries(byAcc)) {
      await dispatchAction({ accountId: acc, threadIds: tids, action: { kind } as never });
    }
    clear();
  };
  return (
    <div style={{ display: 'flex', gap: 8 }}>
      <button onClick={() => void act('archive')} className="sift-chip-btn">
        Archive
      </button>
      <button onClick={() => void act('trash')} className="sift-chip-btn">
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

function AttachmentStrip({
  messageId,
  attachments,
}: {
  messageId: string;
  attachments: { id: string; filename?: string | null; mime: string; size: number }[];
}) {
  const files = attachments.filter((a) => a.filename);
  if (!files.length) return null;
  return (
    <div style={{ display: 'flex', gap: 8, margin: '8px 0', flexWrap: 'wrap' }}>
      {files.map((a) => (
        <div
          key={a.id}
          style={{
            display: 'flex',
            alignItems: 'stretch',
            height: 56,
            border: '1px solid var(--border)',
            borderRadius: 8,
            background: 'var(--n1)',
            overflow: 'hidden',
          }}
        >
          <button
            onClick={() => void api.attachments_open(a.id)}
            title={`Open ${a.filename}`}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              padding: '0 12px',
              border: 'none',
              background: 'none',
              color: 'var(--fg)',
              cursor: 'pointer',
              fontSize: 12,
              maxWidth: 280,
            }}
          >
            {a.mime.startsWith('image/') ? (
              <img
                src={`sift-att://${messageId}/${a.id}`}
                alt=""
                style={{ width: 32, height: 32, objectFit: 'cover', borderRadius: 4 }}
              />
            ) : (
              <Paperclip size={16} />
            )}
            <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {a.filename} <span style={{ color: 'var(--fg-3)' }}>{(a.size / 1024).toFixed(0)}KB</span>
            </span>
          </button>
          <button
            onClick={() => void api.attachments_save_as(a.id)}
            title="Save to disk"
            aria-label={`Save ${a.filename}`}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              width: 40,
              border: 'none',
              borderLeft: '1px solid var(--border)',
              background: 'none',
              color: 'var(--fg-2)',
              cursor: 'pointer',
            }}
          >
            <Download size={15} />
          </button>
        </div>
      ))}
    </div>
  );
}

function UnsubPill({ messageId }: { messageId: string }) {
  return (
    <button
      onClick={() => void api.unsubscribe(messageId)}
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
