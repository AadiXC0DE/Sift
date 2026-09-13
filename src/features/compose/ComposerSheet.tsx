import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useEditor, EditorContent, type ChainedCommands } from '@tiptap/react';
import StarterKit from '@tiptap/starter-kit';
import Link from '@tiptap/extension-link';
import Placeholder from '@tiptap/extension-placeholder';
import { Bold, Italic, List, ListOrdered, Link2, Paperclip, Quote, X } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { Sheet } from '../../ui/Sheet';
import { Button } from '../../ui/Button';
import { Spinner } from '../../ui/Spinner';
import { api } from '../../app/ipc/commands';
import type { Address, AttachmentRef, Contact, Draft, ThreadRef } from '../../app/ipc/types';
import { useAccounts } from '../../stores/accountsStore';
import { useSettings } from '../../stores/settingsStore';
import { useView } from '../../stores/viewStore';
import { firstInvalid, mergeRecipients, parseAddressList } from './recipients';
import { SignatureBlock, applySignature, composedBodyHtml, signatureSource } from './signature';
import { basename, errorReason, stagingFailureMessage, useNativeDropListener } from './attachDrop';
import { escapeHtml, htmlToPlainText } from './plainText';
import { readLastSender, rememberLastSender, resolveComposeAccount } from './composeAccount';

type Field = 'to' | 'cc' | 'bcc';
const FIELDS: Field[] = ['to', 'cc', 'bcc'];

export function ComposerSheet({
  mode,
  thread,
  onClose,
}: {
  mode: string;
  thread?: ThreadRef;
  onClose: () => void;
}) {
  useEffect(() => {
    (window as unknown as { __composeOpen?: boolean }).__composeOpen = true;
    return () => {
      (window as unknown as { __composeOpen?: boolean }).__composeOpen = false;
    };
  }, []);

  const accounts = useAccounts((s) => s.accounts);
  const included = useAccounts((s) => s.included);
  const accountScope = useView((s) => s.accountScope);
  const settings = useSettings((s) => s.settings);

  const enabledIds = useMemo(
    () => accounts.filter((a) => included[a.id] !== false).map((a) => a.id),
    [accounts, included],
  );

  const [to, setTo] = useState<Address[]>([]);
  const [cc, setCc] = useState<Address[]>([]);
  const [bcc, setBcc] = useState<Address[]>([]);
  const [texts, setTexts] = useState<Record<Field, string>>({ to: '', cc: '', bcc: '' });
  const [showCc, setShowCc] = useState(false);
  const [showBcc, setShowBcc] = useState(false);
  const [subject, setSubject] = useState('');
  const [from, setFrom] = useState('');
  const [atts, setAtts] = useState<AttachmentRef[]>([]);
  const [pending, setPending] = useState<{ path: string; name: string }[]>([]);
  const [localId, setLocalId] = useState<string | undefined>(undefined);
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  const [plainText, setPlainText] = useState(false);
  const [linkOpen, setLinkOpen] = useState(false);
  const [linkUrl, setLinkUrl] = useState('');
  const [escapeOwned, setEscapeOwned] = useState(false);
  const [, bumpToolbar] = useState(0);

  const rootRef = useRef<HTMLDivElement>(null);
  const saveT = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const stageSeq = useRef(0);
  const sigAppliedRef = useRef<string | null>(null);
  const plainRef = useRef(false);
  const closedRef = useRef(false);
  const layers = useRef(new Map<string, () => void>());

  // Latest-render accessors for listeners that must not re-register (native drop).
  const attsRef = useRef(atts);
  attsRef.current = atts;
  const editingRef = useRef({ to, cc, bcc, subject, from, texts, localId, pending });
  editingRef.current = { to, cc, bcc, subject, from, texts, localId, pending };

  const editor = useEditor({
    extensions: [
      StarterKit.configure({ heading: false }),
      Link.configure({ openOnClick: false }),
      Placeholder.configure({ placeholder: 'Write…' }),
      SignatureBlock,
    ],
    content: '',
    editorProps: {
      // Plain-text mode never lets pasted markup carry formatting in.
      transformPastedHTML: (html) => (plainRef.current ? escapeHtml(htmlToPlainText(html)) : html),
    },
  });

  useEffect(() => {
    plainRef.current = plainText;
    editor?.setOptions({ enableInputRules: !plainText, enablePasteRules: !plainText });
    if (plainText && editor) editor.chain().unsetAllMarks().run();
  }, [plainText, editor]);

  useEffect(() => {
    if (!editor) return;
    const refresh = () => bumpToolbar((n) => n + 1);
    editor.on('transaction', refresh);
    return () => {
      editor.off('transaction', refresh);
    };
  }, [editor]);

  // ---- account selection -------------------------------------------------
  useEffect(() => {
    if (mode !== 'new' || from || !accounts.length) return;
    const id = resolveComposeAccount({
      scope: accountScope,
      accounts,
      enabledIds,
      lastSender: readLastSender(),
    });
    if (id) setFrom(id);
  }, [mode, from, accounts, accountScope, enabledIds]);

  const changeFrom = (id: string) => {
    setFrom(id);
    rememberLastSender(id);
    sigAppliedRef.current = id;
    if (editor) applySignature(editor, signatureSource(accounts, id));
  };

  // New messages get their account's signature once; replies get theirs from
  // the prefill (or here, if the account list arrived after the thread). Both
  // paths mark the account so a re-render cannot duplicate the block.
  const contentReadyRef = useRef(mode === 'new');
  useEffect(() => {
    if (!editor || !from || !accounts.length || sigAppliedRef.current === from) return;
    if (!contentReadyRef.current) return;
    sigAppliedRef.current = from;
    applySignature(editor, signatureSource(accounts, from));
  }, [editor, from, accounts]);

  // ---- reply / forward prefill ------------------------------------------
  useEffect(() => {
    if (!editor) return;
    if (mode !== 'reply' && mode !== 'reply_all' && mode !== 'forward') return;
    if (!thread) return;
    let cancelled = false;
    api
      .thread_get(thread.accountId, thread.threadId)
      .then((d) => {
        if (cancelled) return;
        const last = d.messages[d.messages.length - 1];
        if (!last) return;
        if (mode === 'forward') {
          setSubject(`Fwd: ${last.subject}`);
        } else {
          setSubject(last.subject.startsWith('Re:') ? last.subject : `Re: ${last.subject}`);
          setTo([{ e: last.from.e, n: last.from.n }]);
          if (mode === 'reply_all' && last.cc.length) {
            setCc(last.cc);
            setShowCc(true);
          }
        }
        setFrom(d.accountId);
        const raw = signatureSource(accounts, d.accountId);
        // If the account list has not arrived yet, leave the flag unset so the
        // signature effect applies once the sending account is known.
        if (raw || accounts.length) sigAppliedRef.current = d.accountId;
        const quote = `<details class="sift-quote"><summary>•••</summary><div>${last.snippet}</div></details>`;
        editor.commands.setContent(composedBodyHtml(raw, quote));
        editor.commands.setTextSelection(1);
        contentReadyRef.current = true;
      })
      .catch(() => {
        /* offline: open an empty composer rather than blocking reply */
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor, mode, thread?.accountId, thread?.threadId]);

  useEffect(() => {
    if (!editor) return;
    const t = setTimeout(() => editor.commands.focus('start'), 0);
    return () => clearTimeout(t);
  }, [editor]);

  // ---- draft persistence -------------------------------------------------
  const persist = useCallback(
    async (
      html?: string,
      overrides?: {
        to?: Address[];
        cc?: Address[];
        bcc?: Address[];
        atts?: AttachmentRef[];
        fromId?: string;
      },
    ): Promise<Draft | null> => {
      clearTimeout(saveT.current);
      saveT.current = undefined;
      const cur = editingRef.current;
      const accountId = overrides?.fromId ?? cur.from;
      if (!accountId) return null;
      const d: Draft = {
        localId: cur.localId,
        accountId,
        threadId: thread?.threadId,
        mode,
        toJson: overrides?.to ?? cur.to,
        ccJson: overrides?.cc ?? cur.cc,
        bccJson: overrides?.bcc ?? cur.bcc,
        subject: cur.subject,
        bodyHtml: html ?? editor?.getHTML() ?? '',
        attachmentsJson: overrides?.atts ?? attsRef.current,
      };
      try {
        const saved = await api.drafts_upsert(d);
        if (saved?.localId && !closedRef.current) setLocalId(saved.localId);
        return saved;
      } catch {
        return null; // offline: keep the local draft in memory
      }
    },
    [editor, mode, thread?.threadId],
  );

  const lastHtmlRef = useRef('');
  const persistRef = useRef(persist);
  persistRef.current = persist;

  useEffect(() => {
    if (!editor) return;
    const h = () => {
      lastHtmlRef.current = editor.getHTML();
      clearTimeout(saveT.current);
      saveT.current = setTimeout(() => {
        saveT.current = undefined;
        void persistRef.current(lastHtmlRef.current);
      }, 300);
    };
    editor.on('update', h);
    return () => {
      editor.off('update', h);
    };
  }, [editor]);

  // A queued autosave must never fire after the sheet is gone: flush it now
  // (the reader may have closed the sheet without going through `close`).
  useEffect(
    () => () => {
      const pending = saveT.current;
      clearTimeout(pending);
      saveT.current = undefined;
      if (pending && !closedRef.current) void persistRef.current(lastHtmlRef.current);
    },
    [],
  );

  const close = () => {
    if (closedRef.current) return;
    closedRef.current = true;
    clearTimeout(saveT.current);
    saveT.current = undefined;
    void persist(lastHtmlRef.current || editor?.getHTML());
    onClose();
  };

  // ---- attachments -------------------------------------------------------
  const stage = async (paths: string[]) => {
    if (!paths.length) return;
    const batch = paths.map((p) => ({ path: p, name: basename(p) }));
    const seq = ++stageSeq.current;
    setPending((x) => [...x, ...batch]);
    try {
      const refs = await api.attachments_add_from_paths(paths);
      if (stageSeq.current !== seq) return;
      const next = [...attsRef.current, ...refs]; // refresh chips from the command result
      setAtts(next);
      if (refs.length < batch.length) {
        toast.error(`Couldn't attach ${batch.length - refs.length} of ${batch.length} files`);
      }
      void persist(editor?.getHTML(), { atts: next });
    } catch (e) {
      // Existing attachments stay; the failure names the file and the reason.
      if (stageSeq.current === seq) toast.error(stagingFailureMessage(paths, e));
    } finally {
      setPending((x) => x.filter((p) => !batch.some((b) => b.path === p.path)));
    }
  };

  useNativeDropListener(rootRef, (paths) => void stage(paths));

  const pickAttachments = async () => {
    if (pending.length) return;
    try {
      const picked = await openDialog({ multiple: true });
      if (!picked) return; // picker cancel is silent
      const paths = (Array.isArray(picked) ? picked : [picked]).map(String).filter(Boolean);
      await stage(paths);
    } catch (e) {
      toast.error(`Couldn't open the file picker: ${errorReason(e)}`);
    }
  };

  const removeAttachment = (index: number) => {
    const next = atts.filter((_, i) => i !== index);
    setAtts(next);
    void persist(editor?.getHTML(), { atts: next });
  };

  // ---- recipients --------------------------------------------------------
  const valuesRef = useRef({ to, cc, bcc });
  valuesRef.current = { to, cc, bcc };
  const setters: Record<Field, (v: Address[]) => void> = { to: setTo, cc: setCc, bcc: setBcc };

  const commitField = (field: Field) => {
    const raw = editingRef.current.texts[field];
    if (!raw.trim()) return valuesRef.current[field];
    const parsed = parseAddressList(raw);
    const merged = parsed.length
      ? mergeRecipients(valuesRef.current[field], parsed)
      : valuesRef.current[field];
    setters[field](merged);
    setTexts((t) => ({ ...t, [field]: '' }));
    return merged;
  };

  /** Commit any half-typed address text and return the recipients as they will be sent. */
  const snapshotRecipients = (): Record<Field, Address[]> => ({
    to: commitField('to'),
    cc: commitField('cc'),
    bcc: commitField('bcc'),
  });

  const setLayer = useCallback((key: string, open: boolean, dismiss: () => void) => {
    if (open) layers.current.set(key, dismiss);
    else layers.current.delete(key);
    setEscapeOwned(layers.current.size > 0);
  }, []);

  useEffect(() => {
    setLayer('link', linkOpen, () => setLinkOpen(false));
  }, [linkOpen, setLayer]);

  // ---- sending -----------------------------------------------------------
  const send = async (archive = false) => {
    if (sending) return;
    if (pending.length) {
      toast.error('Attachments are still being added');
      return;
    }
    const r = snapshotRecipients();
    const all = [...r.to, ...r.cc, ...r.bcc];
    const invalid = firstInvalid(all);
    if (invalid) {
      toast.error(`Invalid address: ${invalid.e}`);
      return;
    }
    if (!all.length) {
      toast.error('Add a recipient');
      return;
    }
    setSending(true);
    const html = editor?.getHTML() ?? '';
    const saved =
      (await api
        .drafts_upsert({
          localId,
          accountId: from,
          threadId: thread?.threadId,
          mode,
          toJson: r.to,
          ccJson: r.cc,
          bccJson: r.bcc,
          subject,
          bodyHtml: html,
          attachmentsJson: atts,
        })
        .catch(() => null)) ?? null;
    const lid = saved?.localId ?? localId;
    if (!lid) {
      toast.error('Send failed. Draft kept');
      setSending(false);
      return;
    }
    rememberLastSender(from);
    try {
      const { op_id } = await api.drafts_send(lid, settings.undoSendDelay * 1000);
      setSent(true);
      setTimeout(() => {
        if (!closedRef.current) {
          closedRef.current = true;
          onClose();
        }
        toast('Sent · Undo', {
          action: { label: 'Undo', onClick: () => void api.send_cancel(op_id).catch(() => {}) },
          duration: settings.undoSendDelay * 1000,
        });
        if (archive && thread) {
          import('../actions/dispatch').then(({ dispatchAction }) => {
            void dispatchAction(
              {
                accountId: thread.accountId,
                threadIds: [thread.threadId],
                action: { kind: 'archive' },
              },
              { silent: true },
            );
          });
        }
        void api.drafts_delete(lid).catch(() => {});
      }, 200);
    } catch {
      toast.error('Send failed. Draft kept');
      setSending(false);
    }
  };
  const sendRef = useRef(send);
  sendRef.current = send;
  const pickAttachmentsRef = useRef(pickAttachments);
  pickAttachmentsRef.current = pickAttachments;
  const closeRef = useRef(close);
  closeRef.current = close;

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === 'Enter') {
        e.preventDefault();
        void sendRef.current(e.shiftKey);
        return;
      }
      if (mod && e.shiftKey && e.key.toLowerCase() === 'a') {
        e.preventDefault();
        void pickAttachmentsRef.current();
        return;
      }
      if (plainRef.current && mod && ['b', 'i', 'u'].includes(e.key.toLowerCase())) {
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      if (e.key === 'Escape') {
        // An outer handler (App) consults `data-compose-escape`; dismiss the
        // innermost layer here and let the next Escape close the sheet.
        if (layers.current.size) {
          e.preventDefault();
          e.stopPropagation();
          const dismissers = [...layers.current.values()];
          layers.current.clear();
          setEscapeOwned(false);
          dismissers.forEach((fn) => fn());
          return;
        }
        e.preventDefault();
        e.stopPropagation();
        closeRef.current();
      }
    };
    window.addEventListener('keydown', h, true);
    return () => window.removeEventListener('keydown', h, true);
  }, []);

  // ---- toolbar -----------------------------------------------------------
  const runCmd = (fn: (chain: ChainedCommands) => ChainedCommands) => {
    if (!editor) return;
    fn(editor.chain().focus()).run();
  };

  const applyLink = () => {
    if (!editor) return;
    const href = linkUrl.trim();
    if (!href) editor.chain().focus().extendMarkRange('link').unsetLink().run();
    else editor.chain().focus().extendMarkRange('link').setLink({ href }).run();
    setLinkOpen(false);
    setLinkUrl('');
  };

  const openLink = () => {
    if (!editor) return;
    setLinkUrl((editor.getAttributes('link').href as string | undefined) ?? '');
    setLinkOpen((v) => !v);
  };

  const attsSize = atts.reduce((n, a) => n + a.size, 0);

  return (
    <Sheet onDismiss={close}>
      <div
        ref={rootRef}
        data-compose-escape={escapeOwned ? '1' : undefined}
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
              onChange={(e) => changeFrom(e.target.value)}
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
          <button onClick={close} aria-label="Close composer" style={iconButtonStyle}>
            <X size={16} />
          </button>
        </div>

        {FIELDS.map((field) =>
          field === 'to' || (field === 'cc' ? showCc : showBcc) ? (
            <RecipientRow
              key={field}
              label={field === 'to' ? 'To' : field === 'cc' ? 'Cc' : 'Bcc'}
              values={field === 'to' ? to : field === 'cc' ? cc : bcc}
              text={texts[field]}
              onText={(v) => setTexts((t) => ({ ...t, [field]: v }))}
              onChange={setters[field]}
              onCommit={() => commitField(field)}
              accountId={from}
              onLayer={setLayer}
            />
          ) : null,
        )}
        <div style={{ display: 'flex', gap: 8, padding: '0 16px 4px' }}>
          {!showCc && (
            <button onClick={() => setShowCc(true)} style={linkButtonStyle} aria-label="Show Cc recipients">
              Cc
            </button>
          )}
          {!showBcc && (
            <button onClick={() => setShowBcc(true)} style={linkButtonStyle} aria-label="Show Bcc recipients">
              Bcc
            </button>
          )}
        </div>

        <input
          value={subject}
          onChange={(e) => setSubject(e.target.value)}
          placeholder="Subject"
          aria-label="Subject"
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

        <div
          role="toolbar"
          aria-label="Formatting"
          style={{
            display: 'flex',
            gap: 2,
            alignItems: 'center',
            padding: '6px 12px',
            borderBottom: '1px solid var(--border)',
          }}
        >
          <ToolbarButton
            label="Bold"
            shortcut="⌘B"
            disabled={plainText}
            active={!!editor?.isActive('bold')}
            onClick={() => runCmd((c) => c.toggleBold())}
          >
            <Bold size={15} />
          </ToolbarButton>
          <ToolbarButton
            label="Italic"
            shortcut="⌘I"
            disabled={plainText}
            active={!!editor?.isActive('italic')}
            onClick={() => runCmd((c) => c.toggleItalic())}
          >
            <Italic size={15} />
          </ToolbarButton>
          <ToolbarButton
            label="Bulleted list"
            disabled={plainText}
            active={!!editor?.isActive('bulletList')}
            onClick={() => runCmd((c) => c.toggleBulletList())}
          >
            <List size={15} />
          </ToolbarButton>
          <ToolbarButton
            label="Numbered list"
            disabled={plainText}
            active={!!editor?.isActive('orderedList')}
            onClick={() => runCmd((c) => c.toggleOrderedList())}
          >
            <ListOrdered size={15} />
          </ToolbarButton>
          <ToolbarButton
            label="Quote"
            disabled={plainText}
            active={!!editor?.isActive('blockquote')}
            onClick={() => runCmd((c) => c.toggleBlockquote())}
          >
            <Quote size={15} />
          </ToolbarButton>
          <ToolbarButton
            label="Link"
            shortcut="⌘K"
            disabled={plainText}
            active={!!editor?.isActive('link')}
            onClick={openLink}
          >
            <Link2 size={15} />
          </ToolbarButton>
          <span style={{ flex: 1 }} />
          <ToolbarButton
            label="Plain text"
            pressed={plainText}
            onClick={() => setPlainText((v) => !v)}
            text
          />
        </div>

        {linkOpen && (
          <div
            style={{ display: 'flex', gap: 6, padding: '6px 12px', borderBottom: '1px solid var(--border)' }}
          >
            <input
              autoFocus
              value={linkUrl}
              onChange={(e) => setLinkUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  applyLink();
                }
              }}
              placeholder="https://…"
              aria-label="Link URL"
              style={{
                flex: 1,
                fontSize: 12,
                padding: '4px 8px',
                borderRadius: 6,
                border: '1px solid var(--border)',
                background: 'var(--bg-raised)',
                color: 'var(--fg)',
              }}
            />
            <Button size="sm" onClick={applyLink}>
              Apply
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                editor?.chain().focus().extendMarkRange('link').unsetLink().run();
                setLinkOpen(false);
                setLinkUrl('');
              }}
            >
              Remove
            </Button>
          </div>
        )}

        <div style={{ flex: 1, overflowY: 'auto', padding: '12px 16px', minHeight: 0 }}>
          <style>{`
            .tiptap { min-height: 120px; outline: none; font-size: 14px; line-height: 1.55; color: var(--fg); }
            .tiptap:focus { outline: none; }
            .tiptap p { margin: 0 0 8px; }
            .tiptap.is-empty::before { content: attr(data-placeholder); color: var(--fg-3); float: left; height: 0; pointer-events: none; }
            .tiptap a { color: var(--accent); }
            .tiptap blockquote { border-left: 2px solid var(--border-strong); margin: 8px 0; padding: 4px 12px; color: var(--fg-2); }
            .tiptap ul, .tiptap ol { padding-left: 20px; margin: 0 0 8px; }
            .tiptap code { font-family: var(--font-mono); font-size: 13px; background: var(--n2); border-radius: 4px; padding: 0 4px; }
            .tiptap [data-sift-signature] { color: var(--fg-2); }
          `}</style>
          <EditorContent editor={editor} />
        </div>

        {(atts.length > 0 || pending.length > 0) && (
          <div
            style={{
              display: 'flex',
              gap: 8,
              padding: '8px 16px',
              borderTop: '1px solid var(--border)',
              flexWrap: 'wrap',
              alignItems: 'center',
            }}
            aria-label="Attachments"
          >
            {atts.map((a, i) => (
              <span key={`${a.path}:${i}`} style={chipStyle}>
                <Paperclip size={12} />
                {a.name}
                <span style={{ color: 'var(--fg-3)' }}>{(a.size / 1024).toFixed(0)}KB</span>
                <button
                  onClick={() => removeAttachment(i)}
                  disabled={sending}
                  aria-label={`Remove ${a.name}`}
                  style={chipRemoveStyle}
                >
                  <X size={11} />
                </button>
              </span>
            ))}
            {pending.map((p) => (
              <span key={p.path} style={{ ...chipStyle, opacity: 0.7 }} aria-busy="true">
                <Spinner size={12} />
                {p.name}
              </span>
            ))}
            {attsSize > 0 && (
              <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>
                {(attsSize / 1024).toFixed(0)}KB total
              </span>
            )}
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
          <Button
            variant="primary"
            onClick={() => void send()}
            disabled={sending || pending.length > 0}
            style={{
              padding: '8px 20px',
              height: 36,
              fontWeight: 600,
              background: sent ? 'var(--success)' : 'var(--accent)',
            }}
          >
            {sent ? '✓' : sending ? 'Sending…' : 'Send ⌘↵'}
          </Button>
          <Button
            onClick={() => void send(true)}
            disabled={sending || pending.length > 0}
            title="Send and archive (⌘⇧↵)"
          >
            Send & archive
          </Button>
          <Button
            onClick={() => void pickAttachments()}
            disabled={sending || pending.length > 0}
            title="Attach (⌘⇧A)"
          >
            Attach
          </Button>
          <span style={{ flex: 1 }} />
          <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>Esc saves draft</span>
        </div>
      </div>
    </Sheet>
  );
}

const iconButtonStyle: React.CSSProperties = {
  background: 'none',
  border: 'none',
  cursor: 'pointer',
  color: 'var(--fg-2)',
  display: 'inline-flex',
  alignItems: 'center',
};

const linkButtonStyle: React.CSSProperties = {
  background: 'none',
  border: 'none',
  color: 'var(--fg-3)',
  cursor: 'pointer',
  fontSize: 12,
  padding: 0,
};

const chipStyle: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 4,
  fontSize: 12,
  background: 'var(--n2)',
  borderRadius: 6,
  padding: '4px 8px',
};

const chipRemoveStyle: React.CSSProperties = {
  background: 'none',
  border: 'none',
  cursor: 'pointer',
  color: 'var(--fg-2)',
  display: 'inline-flex',
  alignItems: 'center',
};

function ToolbarButton({
  label,
  shortcut,
  active,
  pressed,
  disabled,
  onClick,
  text,
  children,
}: {
  label: string;
  shortcut?: string;
  active?: boolean;
  pressed?: boolean;
  disabled?: boolean;
  onClick: () => void;
  text?: boolean;
  children?: React.ReactNode;
}) {
  const on = active || pressed;
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      aria-pressed={!!on}
      title={shortcut ? `${label} (${shortcut})` : label}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 4,
        height: 26,
        padding: text ? '0 8px' : 0,
        width: text ? undefined : 26,
        justifyContent: 'center',
        borderRadius: 6,
        border: '1px solid transparent',
        background: on ? 'var(--accent-soft, var(--n3))' : 'transparent',
        color: disabled ? 'var(--fg-3)' : 'var(--fg)',
        cursor: disabled ? 'default' : 'pointer',
        fontSize: 12,
        opacity: disabled ? 0.5 : 1,
      }}
    >
      {children}
      {text ? label : null}
    </button>
  );
}

function RecipientRow({
  label,
  values,
  text,
  onText,
  onChange,
  onCommit,
  accountId,
  onLayer,
}: {
  label: string;
  values: Address[];
  text: string;
  onText: (v: string) => void;
  onChange: (v: Address[]) => void;
  onCommit: () => Address[];
  accountId: string;
  onLayer: (key: string, open: boolean, dismiss: () => void) => void;
}) {
  const [sugs, setSugs] = useState<Contact[]>([]);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [selected, setSelected] = useState<number | null>(null);
  const seq = useRef(0);
  const listId = useId();

  const dismiss = () => {
    setOpen(false);
    setActive(-1);
  };
  const dismissRef = useRef(dismiss);
  dismissRef.current = dismiss;

  useLayoutEffect(() => {
    onLayer(label, open, () => dismissRef.current());
  }, [open, label, onLayer]);

  // Suggestions come from the sending account only, debounced, with stale
  // responses dropped; offline simply yields no suggestions.
  useEffect(() => {
    const q = text.trim();
    if (!q || !accountId) {
      setSugs([]);
      dismiss();
      return;
    }
    const id = ++seq.current;
    const t = setTimeout(() => {
      api
        .contacts_suggest(accountId, q, 6)
        .then((r) => {
          if (seq.current !== id) return;
          setSugs(r);
          setOpen(r.length > 0);
          setActive(r.length ? 0 : -1);
        })
        .catch(() => {
          if (seq.current !== id) return;
          setSugs([]);
          setOpen(false);
          setActive(-1);
        });
    }, 120);
    return () => clearTimeout(t);
  }, [text, accountId]);

  const pick = (i: number) => {
    const c = sugs[i];
    if (!c) return;
    onChange(mergeRecipients(values, [{ e: c.email, n: c.name ?? undefined }]));
    onText('');
    setSugs([]);
    setOpen(false);
    setActive(-1);
  };

  const removeAt = (i: number) => {
    onChange(values.filter((_, j) => j !== i));
    setSelected(null);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (open && sugs.length) setActive((a) => Math.min(a + 1, sugs.length - 1));
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (open && sugs.length) setActive((a) => Math.max(a - 1, 0));
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (open && active >= 0) pick(active);
      else onCommit();
      return;
    }
    if (e.key === ',' || e.key === ';') {
      e.preventDefault();
      onCommit();
      return;
    }
    if (e.key === 'Tab') {
      onCommit();
      return;
    }
    if (e.key === 'Escape') {
      if (open) {
        e.preventDefault();
        e.stopPropagation();
        dismiss();
      } else if (selected != null) {
        e.preventDefault();
        e.stopPropagation();
        setSelected(null);
      }
      return;
    }
    if (e.key === 'Backspace') {
      if (text) return;
      e.preventDefault();
      if (selected != null && values[selected]) removeAt(selected);
      else if (values.length) setSelected(values.length - 1);
      return;
    }
    if (e.key === 'Delete') {
      if (!text && selected != null && values[selected]) {
        e.preventDefault();
        removeAt(selected);
      }
      return;
    }
    if (e.key === 'ArrowLeft' && text === '') {
      if (values.length) {
        e.preventDefault();
        setSelected((s) => (s == null ? values.length - 1 : Math.max(s - 1, 0)));
      }
      return;
    }
    if (e.key === 'ArrowRight' && selected != null) {
      e.preventDefault();
      setSelected((s) => (s == null || s >= values.length - 1 ? null : s + 1));
    }
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
      <div role="group" style={{ display: 'flex', gap: 4, flexWrap: 'wrap', flex: 1, alignItems: 'center' }}>
        {values.map((a, i) => {
          const invalid = !!a.e && firstInvalid([a]) !== null;
          return (
            <span
              key={`${a.e}:${i}`}
              role="button"
              tabIndex={-1}
              aria-label={`${a.n ? `${a.n} <${a.e}>` : a.e}${invalid ? ' (invalid address)' : ''}`}
              aria-invalid={invalid || undefined}
              aria-selected={selected === i}
              onMouseDown={(e) => {
                e.preventDefault();
                setSelected(i);
              }}
              style={{
                fontSize: 12,
                background:
                  selected === i
                    ? 'var(--accent-soft, var(--n3))'
                    : invalid
                      ? 'color-mix(in oklab, var(--danger) 12%, transparent)'
                      : 'var(--n2)',
                color: invalid ? 'var(--danger)' : 'var(--fg)',
                boxShadow: selected === i ? '0 0 0 1px var(--accent)' : undefined,
                borderRadius: 999,
                padding: '2px 8px',
                display: 'inline-flex',
                gap: 4,
                alignItems: 'center',
                cursor: 'default',
              }}
            >
              {a.n ? `${a.n} <${a.e}>` : a.e}
              <button
                type="button"
                onClick={() => removeAt(i)}
                aria-label={`Remove ${a.n ?? a.e}`}
                style={chipRemoveStyle}
              >
                <X size={10} />
              </button>
            </span>
          );
        })}
        <input
          value={text}
          onChange={(e) => {
            onText(e.target.value);
            setSelected(null);
          }}
          onKeyDown={onKeyDown}
          onBlur={() => {
            onCommit();
            setTimeout(() => dismissRef.current(), 150);
          }}
          onPaste={(e) => {
            const t = e.clipboardData.getData('text');
            if (t.includes(',') || t.includes(';') || t.includes('@')) {
              e.preventDefault();
              const parsed = parseAddressList(t);
              if (parsed.length) onChange(mergeRecipients(values, parsed));
            }
          }}
          role="combobox"
          aria-expanded={open}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={open && active >= 0 ? `${listId}-${active}` : undefined}
          aria-label={`${label} recipients`}
          style={{
            border: 'none',
            outline: 'none',
            flex: 1,
            minWidth: 120,
            fontSize: 13,
            background: 'transparent',
            color: 'var(--fg)',
          }}
        />
      </div>
      {open && sugs.length > 0 && (
        <div
          role="listbox"
          id={listId}
          aria-label={`${label} suggestions`}
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
            <div
              key={`${s.email}:${i}`}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={active === i}
              onMouseDown={(e) => {
                e.preventDefault();
                pick(i);
              }}
              style={{
                display: 'block',
                width: '100%',
                textAlign: 'left',
                background: active === i ? 'var(--n3)' : 'none',
                border: 'none',
                padding: '6px 8px',
                fontSize: 13,
                cursor: 'pointer',
                borderRadius: 6,
                color: 'var(--fg)',
              }}
            >
              {s.name ? `${s.name} <${s.email}>` : s.email}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
