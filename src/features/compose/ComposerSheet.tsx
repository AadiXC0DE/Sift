import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useEditor, EditorContent } from '@tiptap/react';
import StarterKit from '@tiptap/starter-kit';
import Link from '@tiptap/extension-link';
import Placeholder from '@tiptap/extension-placeholder';
import { Sheet } from '../../ui/Sheet';
import { api } from '../../app/ipc/commands';
import type { Address, AttachmentRef, Draft } from '../../app/ipc/types';
import { useAccounts } from '../../stores/accountsStore';
import { useSettings } from '../../stores/settingsStore';
import { toast } from 'sonner';

export function ComposerSheet({
  mode,
  threadId,
  onClose,
}: {
  mode: string;
  threadId?: string;
  onClose: () => void;
}) {
  const accounts = useAccounts((s) => s.accounts);
  const scope = useAccounts((s) => s.accounts[0]?.id ?? '');
  void scope;
  const settings = useSettings((s) => s.settings);
  const [to, setTo] = useState<Address[]>([]);
  const [cc, setCc] = useState<Address[]>([]);
  const [bcc, setBcc] = useState<Address[]>([]);
  const [showCc, setShowCc] = useState(false);
  const [subject, setSubject] = useState('');
  const [from, setFrom] = useState(accounts[0]?.id ?? '');
  const [atts, setAtts] = useState<AttachmentRef[]>([]);
  const [localId, setLocalId] = useState<string | undefined>(undefined);
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  const saveT = useRef<ReturnType<typeof setTimeout> | null>(null);

  const editor = useEditor({
    extensions: [
      StarterKit.configure({ heading: false }),
      Link,
      Placeholder.configure({ placeholder: 'Write…' }),
    ],
    content: '',
  });

  // prefill reply
  useEffect(() => {
    if ((mode === 'reply' || mode === 'reply_all' || mode === 'forward') && threadId) {
      const acc = (window as unknown as { __lastAccount?: string }).__lastAccount ?? accounts[0]?.id ?? '';
      if (acc) {
        api
          .thread_get(acc, threadId)
          .then((d) => {
            const last = d.messages[d.messages.length - 1];
            if (!last) return;
            if (mode === 'forward') {
              setSubject(`Fwd: ${last.subject}`);
            } else {
              setSubject(last.subject.startsWith('Re:') ? last.subject : `Re: ${last.subject}`);
              setTo([{ e: last.from.e, n: last.from.n }]);
              if (mode === 'reply_all') setCc(last.cc);
            }
            setFrom(d.accountId);
            // signature above quote
            const sig = accounts.find((a) => a.id === d.accountId)?.signature_html ?? '';
            const quote = `<details class="sift-quote"><summary>•••</summary><div>${last.snippet}</div></details>`;
            editor?.commands.setContent(`${sig ? `<div>${sig}</div>` : ''}<p></p>${quote}`);
          })
          .catch(() => {});
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, threadId]);

  const save = useCallback(
    async (html: string) => {
      if (!from) return;
      const d: Draft = {
        localId: localId,
        accountId: from,
        threadId,
        mode,
        toJson: to,
        ccJson: cc,
        bccJson: bcc,
        subject,
        bodyHtml: html,
        attachmentsJson: atts,
      };
      try {
        const saved = await api.drafts_upsert(d);
        setLocalId(saved.localId);
      } catch {
        /* offline: keep local */
      }
    },
    [from, threadId, mode, to, cc, bcc, subject, atts, localId],
  );

  useEffect(() => {
    if (!editor) return;
    const h = () => {
      if (saveT.current) clearTimeout(saveT.current);
      saveT.current = setTimeout(() => void save(editor.getHTML()), 300);
    };
    editor.on('update', h);
    return () => {
      editor.off('update', h);
    };
  }, [editor, save]);

  const send = async (archive = false) => {
    if (sending) return;
    const invalid = [...to, ...cc, ...bcc].filter((a) => !a.e.includes('@'));
    if (to.length === 0) {
      toast.error('Add a recipient');
      return;
    }
    if (invalid.length) {
      toast.error(`Invalid address: ${invalid[0].e}`);
      return;
    }
    setSending(true);
    const html = editor?.getHTML() ?? '';
    await save(html);
    // resolve localId freshly
    const d: Draft = {
      localId: localId,
      accountId: from,
      threadId,
      mode,
      toJson: to,
      ccJson: cc,
      bccJson: bcc,
      subject,
      bodyHtml: html,
      attachmentsJson: atts,
    };
    const saved = await api.drafts_upsert(d).catch(() => d);
    const lid = saved.localId ?? localId;
    if (!lid) {
      setSending(false);
      return;
    }
    try {
      const { op_id } = await api.drafts_send(lid, settings.undoSendDelay * 1000);
      setSent(true);
      setTimeout(() => {
        onClose();
        toast('Sent · Undo', {
          action: { label: 'Undo', onClick: () => void api.send_cancel(op_id).catch(() => {}) },
          duration: settings.undoSendDelay * 1000,
        });
        if (archive) {
          // send & archive: archive thread
          import('../actions/dispatch').then(({ dispatchAction }) => {
            if (threadId)
              void dispatchAction(
                { accountId: from, threadIds: [threadId], action: { kind: 'archive' } },
                { silent: true },
              );
          });
        }
        try {
          void api.drafts_delete(lid);
        } catch {
          /* noop */
        }
      }, 200);
    } catch {
      toast.error('Send failed — draft kept');
      setSending(false);
    }
  };

  const attach = async () => {
    // Tauri dialog open
    const { open } = await import('@tauri-apps/plugin-dialog');
    const paths = await open({ multiple: true });
    if (!paths) return;
    const list = Array.isArray(paths) ? paths : [paths];
    const refs = await api.attachments_add_from_paths(list.map(String)).catch(() => [] as AttachmentRef[]);
    // client-side helper for tests
    const { attachments_add_from_paths } = await import('./attachHelper');
    void attachments_add_from_paths;
    setAtts((a) => [...a, ...refs]);
  };

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        void send();
      }
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === 'Enter') {
        e.preventDefault();
        void send(true);
      }
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', h, true);
    return () => window.removeEventListener('keydown', h, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [send]);

  const onDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    const files = Array.from(e.dataTransfer.files);
    if (!files.length) return;
    // In Tauri, file paths available via .path (webkit); fallback: ignore
    const paths = files.map((f) => (f as unknown as { path?: string }).path).filter(Boolean) as string[];
    if (paths.length) {
      const refs = await api.attachments_add_from_paths(paths).catch(() => [] as AttachmentRef[]);
      setAtts((a) => [...a, ...refs]);
    }
  };

  return (
    <Sheet onDismiss={onClose}>
      <div
        onDragOver={(e) => e.preventDefault()}
        onDrop={onDrop}
        style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}
      >
        <div
          style={{
            display: 'flex',
            gap: 8,
            padding: '12px 16px',
            borderBottom: '1px solid var(--border)',
            alignItems: 'center',
          }}
        >
          <span style={{ fontWeight: 600, fontSize: 14, flex: 1 }}>
            {mode === 'new' ? 'New message' : mode === 'forward' ? 'Forward' : 'Reply'}
          </span>
          {accounts.length > 1 && (
            <select
              value={from}
              onChange={(e) => setFrom(e.target.value)}
              aria-label="From account"
              style={{ fontSize: 12 }}
            >
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.email}
                </option>
              ))}
            </select>
          )}
          <button
            onClick={onClose}
            style={{ background: 'none', border: 'none', cursor: 'pointer', fontSize: 16 }}
          >
            ×
          </button>
        </div>
        <RecipientRow label="To" values={to} onChange={setTo} accountId={from} />
        {showCc ? (
          <>
            <RecipientRow label="Cc" values={cc} onChange={setCc} accountId={from} />
            <RecipientRow label="Bcc" values={bcc} onChange={setBcc} accountId={from} />
          </>
        ) : (
          <div style={{ padding: '0 16px' }}>
            <button
              onClick={() => setShowCc(true)}
              style={{
                background: 'none',
                border: 'none',
                color: 'var(--fg-3)',
                cursor: 'pointer',
                fontSize: 12,
              }}
            >
              Cc/Bcc
            </button>
          </div>
        )}
        <input
          value={subject}
          onChange={(e) => setSubject(e.target.value)}
          placeholder="Subject"
          style={{
            border: 'none',
            borderBottom: '1px solid var(--border)',
            padding: '10px 16px',
            fontSize: 14,
            outline: 'none',
            background: 'transparent',
            color: 'var(--fg)',
          }}
        />
        <div style={{ flex: 1, overflowY: 'auto', padding: '12px 16px', minHeight: 0 }}>
          <EditorContent editor={editor} />
        </div>
        {atts.length > 0 && (
          <div
            style={{
              display: 'flex',
              gap: 8,
              padding: '8px 16px',
              borderTop: '1px solid var(--border)',
              flexWrap: 'wrap',
            }}
          >
            {atts.map((a, i) => (
              <span
                key={i}
                style={{ fontSize: 12, background: 'var(--n2)', borderRadius: 6, padding: '4px 8px' }}
              >
                {a.name}{' '}
                <button
                  onClick={() => setAtts((x) => x.filter((_, j) => j !== i))}
                  style={{ background: 'none', border: 'none', cursor: 'pointer' }}
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
        <div
          style={{
            display: 'flex',
            gap: 8,
            padding: '12px 16px',
            borderTop: '1px solid var(--border)',
            alignItems: 'center',
          }}
        >
          <button
            onClick={() => void send()}
            style={{
              background: sent ? 'var(--success)' : 'var(--accent)',
              color: '#fff',
              border: 'none',
              borderRadius: 8,
              padding: '8px 20px',
              fontSize: 13,
              fontWeight: 600,
              cursor: 'pointer',
              transition: 'filter 200ms ease',
              filter: sent ? 'blur(0px)' : 'none',
            }}
          >
            {sent ? '✓' : sending ? 'Sending…' : 'Send ⌘↵'}
          </button>
          <button
            onClick={() => void send(true)}
            title="Send and archive (⌘⇧↵)"
            style={{
              background: 'none',
              border: '1px solid var(--border)',
              borderRadius: 8,
              padding: '8px 12px',
              fontSize: 12,
              cursor: 'pointer',
            }}
          >
            Send & archive
          </button>
          <button
            onClick={() => void attach()}
            title="Attach (⌘⇧A)"
            style={{
              background: 'none',
              border: '1px solid var(--border)',
              borderRadius: 8,
              padding: '8px 12px',
              fontSize: 12,
              cursor: 'pointer',
            }}
          >
            Attach
          </button>
          <span style={{ flex: 1 }} />
          <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>Esc saves draft</span>
        </div>
      </div>
    </Sheet>
  );
}

function RecipientRow({
  label,
  values,
  onChange,
  accountId,
}: {
  label: string;
  values: Address[];
  onChange: (v: Address[]) => void;
  accountId: string;
}) {
  const [input, setInput] = useState('');
  const [sugs, setSugs] = useState<Address[]>([]);
  const [open, setOpen] = useState(false);

  const commit = (raw: string) => {
    const parts = raw
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);
    if (!parts.length) return;
    const addrs: Address[] = parts.map((p) => {
      const m = p.match(/^(.*)<(.+)>$/);
      if (m) return { n: m[1].trim().replace(/"/g, ''), e: m[2].trim() };
      return { e: p };
    });
    onChange([...values, ...addrs]);
    setInput('');
    setOpen(false);
  };

  return (
    <div
      style={{
        display: 'flex',
        gap: 8,
        padding: '8px 16px',
        borderBottom: '1px solid var(--border)',
        alignItems: 'center',
        position: 'relative',
      }}
    >
      <span style={{ fontSize: 12, color: 'var(--fg-3)', width: 28 }}>{label}</span>
      <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap', flex: 1 }}>
        {values.map((a, i) => {
          const invalid = !a.e.includes('@');
          return (
            <span
              key={i}
              style={{
                fontSize: 12,
                background: invalid ? 'color-mix(in oklab, var(--danger) 12%, transparent)' : 'var(--n2)',
                color: invalid ? 'var(--danger)' : 'var(--fg)',
                borderRadius: 999,
                padding: '2px 8px',
                display: 'inline-flex',
                gap: 4,
                alignItems: 'center',
              }}
            >
              {a.n ? `${a.n} <${a.e}>` : a.e}
              <button
                onClick={() => onChange(values.filter((_, j) => j !== i))}
                style={{ background: 'none', border: 'none', cursor: 'pointer' }}
              >
                ×
              </button>
            </span>
          );
        })}
        <input
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            if (e.target.value.length >= 1) {
              api
                .contacts_suggest(accountId, e.target.value, 6)
                .then((r) => {
                  setSugs(r.map((c) => ({ e: c.email, n: c.name ?? undefined })));
                  setOpen(true);
                })
                .catch(() => {});
            } else setOpen(false);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ',') {
              e.preventDefault();
              commit(input);
            }
            if (e.key === 'Backspace' && !input && values.length) onChange(values.slice(0, -1));
          }}
          onBlur={() => {
            if (input) commit(input);
            setTimeout(() => setOpen(false), 150);
          }}
          onPaste={(e) => {
            const t = e.clipboardData.getData('text');
            if (t.includes(',') || t.includes('@')) {
              e.preventDefault();
              commit(t);
            }
          }}
          style={{
            border: 'none',
            outline: 'none',
            flex: 1,
            minWidth: 120,
            fontSize: 13,
            background: 'transparent',
            color: 'var(--fg)',
          }}
          aria-label={`${label} recipients`}
        />
      </div>
      {open && sugs.length > 0 && (
        <div
          style={{
            position: 'absolute',
            top: '100%',
            left: 40,
            background: 'var(--bg-elevated)',
            boxShadow: 'var(--shadow-popover)',
            borderRadius: 8,
            padding: 4,
            zIndex: 10,
            minWidth: 240,
          }}
        >
          {sugs.map((s, i) => (
            <button
              key={i}
              onClick={() => {
                onChange([...values, s]);
                setInput('');
                setOpen(false);
              }}
              style={{
                display: 'block',
                width: '100%',
                textAlign: 'left',
                background: 'none',
                border: 'none',
                padding: '6px 8px',
                fontSize: 13,
                cursor: 'pointer',
              }}
            >
              {s.n ? `${s.n} <${s.e}>` : s.e}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
