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
import { Star, Archive, Trash2, Clock, MoreHorizontal, Reply, Paperclip } from 'lucide-react';

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
      // the list row knows accountId — ThreadList should set it. Fallback: use stored last account.
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
      // bodies
      for (const m of d.messages) {
        if (!exp[m.id]) continue;
        const cached = cacheGet(m.id);
        if (cached) {
          setBodies((b) => ({ ...b, [m.id]: cached }));
          continue;
        }
        api
          .message_body(m.id)
          .then((b) => {
            cacheSet(m.id, b);
            setBodies((prev) => ({ ...prev, [m.id]: b }));
            if (b.state === 'loading') {
              // poll once after 800ms
              setTimeout(() => {
                api
                  .message_body(m.id)
                  .then((b2) => {
                    if (b2.state === 'ready') {
                      cacheSet(m.id, b2);
                      setBodies((p) => ({ ...p, [m.id]: b2 }));
                    }
                  })
                  .catch(() => {});
              }, 800);
            }
          })
          .catch(() => {});
      }
      // prefetch focused±1,+2 (list-level prefetch happens in ThreadList; here warm next messages)
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

  const darkMail = (darkSafe: boolean) => {
    if (settings.darkModeEmails === 'always') return true;
    if (settings.darkModeEmails === 'never') return false;
    return darkSafe && document.documentElement.dataset.theme === 'dark';
  };

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
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <h1 style={{ fontSize: 20, fontWeight: 600, letterSpacing: '-0.01em', flex: 1, margin: 0 }}>
            {detail.subject || '(No subject)'}
          </h1>
          <HeaderActions
            accountId={detail.accountId}
            threadId={detail.id}
            onReply={() => onReply('reply', detail.id)}
          />
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
                    <BodySkeleton />
                  ) : body.state === 'error' ? (
                    <div style={{ color: 'var(--danger)', fontSize: 13 }}>
                      Couldn&apos;t load this message.
                    </div>
                  ) : (
                    <>
                      {body.remoteImageCount > 0 && !body.remoteImagesAllowed && (
                        <div
                          style={{
                            height: 32,
                            background: 'var(--n2)',
                            borderRadius: 6,
                            display: 'flex',
                            alignItems: 'center',
                            gap: 8,
                            padding: '0 12px',
                            fontSize: 12,
                            marginBottom: 8,
                          }}
                        >
                          <span>Images hidden</span>
                          <button
                            onClick={() =>
                              void api.remote_images_load(m.id, false).then((b) => {
                                cacheSet(m.id, b);
                                setBodies((p) => ({ ...p, [m.id]: b }));
                              })
                            }
                            style={{
                              background: 'none',
                              border: 'none',
                              color: 'var(--accent)',
                              cursor: 'pointer',
                              fontSize: 12,
                            }}
                          >
                            Load
                          </button>
                          <button
                            onClick={() =>
                              void api.remote_images_load(m.id, true).then((b) => {
                                cacheSet(m.id, b);
                                setBodies((p) => ({ ...p, [m.id]: b }));
                              })
                            }
                            style={{
                              background: 'none',
                              border: 'none',
                              color: 'var(--accent)',
                              cursor: 'pointer',
                              fontSize: 12,
                            }}
                          >
                            Always load from {m.from.e}
                          </button>
                        </div>
                      )}
                      <MailFrame
                        messageId={m.id}
                        html={body.html}
                        allowed={body.remoteImagesAllowed}
                        dark={darkMail(body.darkSafe)}
                      />
                      {body.trackerCount > 0 && (
                        <div style={{ fontSize: 11, color: 'var(--fg-3)', marginTop: 4 }}>
                          {body.trackerCount} trackers removed
                        </div>
                      )}
                    </>
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

function HeaderActions({
  accountId,
  threadId,
  onReply,
}: {
  accountId: string;
  threadId: string;
  onReply: () => void;
}) {
  void onReply;
  return (
    <div style={{ display: 'flex', gap: 4 }}>
      <button
        title="Star (s)"
        onClick={() =>
          void dispatchAction({ accountId, threadIds: [threadId], action: { kind: 'star', on: true } })
        }
        style={iconBtn}
      >
        <Star size={16} />
      </button>
      <button
        title="Archive (e)"
        onClick={() => void dispatchAction({ accountId, threadIds: [threadId], action: { kind: 'archive' } })}
        style={iconBtn}
      >
        <Archive size={16} />
      </button>
      <button
        title="Trash (#)"
        onClick={() => void dispatchAction({ accountId, threadIds: [threadId], action: { kind: 'trash' } })}
        style={iconBtn}
      >
        <Trash2 size={16} />
      </button>
      <button
        title="Snooze (h)"
        onClick={() =>
          document.dispatchEvent(
            new CustomEvent('sift:snooze', { detail: { accountId, threadIds: [threadId] } }),
          )
        }
        style={iconBtn}
      >
        <Clock size={16} />
      </button>
      <button title="More" style={iconBtn}>
        <MoreHorizontal size={16} />
      </button>
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
      <button
        onClick={() => void act('archive')}
        style={{ padding: '6px 12px', borderRadius: 6, border: '1px solid var(--border)', cursor: 'pointer' }}
      >
        Archive
      </button>
      <button
        onClick={() => void act('trash')}
        style={{ padding: '6px 12px', borderRadius: 6, border: '1px solid var(--border)', cursor: 'pointer' }}
      >
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
  void messageId;
  if (!attachments.length) return null;
  return (
    <div style={{ display: 'flex', gap: 8, margin: '8px 0', flexWrap: 'wrap' }}>
      {attachments
        .filter((a) => a.filename)
        .map((a) => (
          <button
            key={a.id}
            onClick={() => void api.attachments_open(a.id)}
            onContextMenu={(e) => {
              e.preventDefault();
              void api.attachments_save_as(a.id);
            }}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              height: 56,
              padding: '0 12px',
              border: '1px solid var(--border)',
              borderRadius: 8,
              background: 'var(--n1)',
              cursor: 'pointer',
              fontSize: 12,
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
            <span>
              {a.filename} <span style={{ color: 'var(--fg-3)' }}>{(a.size / 1024).toFixed(0)}KB</span>
            </span>
          </button>
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
