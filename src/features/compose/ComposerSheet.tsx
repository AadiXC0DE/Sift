import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useEditor, EditorContent, type ChainedCommands, type Editor } from '@tiptap/react';
import StarterKit from '@tiptap/starter-kit';
import Link from '@tiptap/extension-link';
import Placeholder from '@tiptap/extension-placeholder';
import { Bold, Italic, List, ListOrdered, Link2, Paperclip, Quote, X } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { Sheet } from '../../ui/Sheet';
import { pushSurface, topSurface } from '../../ui/overlayStack';
import { Button } from '../../ui/Button';
import { Spinner } from '../../ui/Spinner';
import { api } from '../../app/ipc/commands';
import type {
  Address,
  AttachmentMeta,
  AttachmentRef,
  Contact,
  Draft,
  MessageBody,
  MessageMeta,
  OperationState,
  SendHandle,
} from '../../app/ipc/types';
import { on } from '../../app/ipc/events';
import { readSiftError } from '../../lib/siftError';
import { COLOR_MIX_SUPPORTED } from '../../lib/css';
import { useAccounts } from '../../stores/accountsStore';
import { useSettings } from '../../stores/settingsStore';
import { useView } from '../../stores/viewStore';
import { firstInvalid, mergeRecipients, parseAddressList } from './recipients';
import { SignatureBlock, applySignature, composedBodyHtml, signatureSource } from './signature';
import { basename, errorReason, stagingFailureMessage, useNativeDropListener } from './attachDrop';
import { escapeHtml, htmlToPlainText } from './plainText';
import { readLastSender, rememberLastSender, resolveComposeAccount } from './composeAccount';
import { DraftSaveQueue, newDraftId, type SaveStatus } from './draftQueue';
import {
  forwardSubject,
  pickParent,
  replyAllRecipients,
  replyRecipients,
  replySubject,
  type ComposeRequest,
} from './replyContext';
import { forwardHtml, quoteHtml } from './quote';
import { QuoteBlock } from './quoteNode';
import { SendLaterMenu } from '../send-later/SendLaterMenu';
import { ScheduleDialog, type ScheduleChoice } from '../send-later/ScheduleDialog';
import { utilities, type ScheduleHandleFields } from '../mail-utilities/ipc';
import { SEND_LATER_COPY, formatInZone, localWallTime, timezoneName } from '../mail-utilities/time';

type Field = 'to' | 'cc' | 'bcc';
const FIELDS: Field[] = ['to', 'cc', 'bcc'];

/** Nothing the editor owns is written before this debounce quiet period. */
const AUTOSAVE_MS = 300;

/**
 * The queued send as the composer reports it (P6.2). The state comes from the
 * outbox, never from the fact that the enqueue call returned: queueing is not
 * sending, and the composer may only claim what the provider actually did.
 */
interface QueuedSend {
  opId: number;
  notBefore: number;
  state: OperationState;
  message: string | null;
  /**
   * Present when this operation is a scheduled send (P8.1). The exact local
   * wall time and zone travel with the row so the banner can name the instant
   * the user chose, not just a UTC timestamp in their current zone.
   */
  schedule?: { localTime: string; timezone: string } | null;
}

export function ComposerSheet({ mode, thread, draftId, onClose }: ComposeRequest & { onClose: () => void }) {
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
  const [activeMode, setActiveMode] = useState(mode);
  const [atts, setAtts] = useState<AttachmentRef[]>([]);
  const [pending, setPending] = useState<{ path: string; name: string }[]>([]);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>('idle');
  const [sending, setSending] = useState(false);
  /** Open state of the "Choose date and time…" dialog (P8.1). */
  const [scheduleOpen, setScheduleOpen] = useState(false);
  /** Open state of the "Change send time" dialog for the queued operation. */
  const [movingSchedule, setMovingSchedule] = useState(false);
  /**
   * The composer never says "Sent" because it queued something (P6.2): this
   * holds the operation the composer is waiting on, and it survives while the
   * app is offline or restarted because it is rebuilt from the outbox.
   */
  const [queued, setQueued] = useState<QueuedSend | null>(null);
  const queuedRef = useRef<QueuedSend | null>(null);
  queuedRef.current = queued;
  const [plainText, setPlainText] = useState(false);
  const [linkOpen, setLinkOpen] = useState(false);
  const [linkUrl, setLinkUrl] = useState('');
  const [escapeOwned, setEscapeOwned] = useState(false);
  const [, bumpToolbar] = useState(0);
  // Reopening a stored draft is a read; the fields stay inert until it lands.
  const [restoring, setRestoring] = useState(!!draftId);
  /** A loaded draft whose body is waiting for the editor to exist. */
  const [pendingDraft, setPendingDraft] = useState<Draft | null>(null);
  const [quoted, setQuoted] = useState<'none' | 'loading' | 'ready' | 'error'>(
    !draftId && thread && mode !== 'new' ? 'loading' : 'ready',
  );
  const [parent, setParent] = useState<MessageMeta | null>(null);
  const [forwardBusy, setForwardBusy] = useState<Record<string, 'loading' | 'error'>>({});

  const rootRef = useRef<HTMLDivElement>(null);
  const stageSeq = useRef(0);
  const sigAppliedRef = useRef<string | null>(null);
  const plainRef = useRef(false);
  const closedRef = useRef(false);
  const layers = useRef(new Map<string, () => void>());

  // A new composer allocates its identity here and never regenerates it, so an
  // asynchronous save can never race a second id into existence (P5.1).
  const localIdRef = useRef<string>('');
  if (!localIdRef.current) localIdRef.current = draftId ?? newDraftId();

  const idFields = useRef<
    Pick<
      Draft,
      'accountId' | 'threadId' | 'inReplyToMessageId' | 'rfcMessageId' | 'remoteDraftId' | 'remoteMessageId'
    >
  >({
    accountId: '',
    threadId: thread?.threadId,
  });
  const subjectRef = useRef('');
  const attsRef = useRef(atts);
  attsRef.current = atts;
  /**
   * Recipients are mirrored into a ref by every mutator (not at render time) so
   * a close/send flush started in the same tick as an edit still sees it.
   */
  const recipientRef = useRef({ to, cc, bcc });
  const editorRef = useRef<Editor | null>(null);
  const modeRef = useRef(activeMode);
  modeRef.current = activeMode;
  /** True once the body may be typed into: a late fetch cannot land after this. */
  const readyRef = useRef(false);

  const accountEmail = useCallback((id: string) => accounts.find((a) => a.id === id)?.email, [accounts]);
  /**
   * Every address this installation owns. The sending account is the verified
   * identity that matters, and the other configured accounts are the user's
   * addresses too — a reply-all must never address them.
   */
  const identitiesOf = useCallback(
    (id: string): string[] => {
      const emails = accounts.map((a) => a.email);
      const own = accounts.find((x) => x.id === id)?.email;
      return own && !emails.includes(own) ? [own, ...emails] : emails;
    },
    [accounts],
  );

  const queueRef = useRef<DraftSaveQueue | null>(null);
  const startQueue = useCallback(
    (initial?: Draft) => {
      if (queueRef.current) return queueRef.current;
      queueRef.current = new DraftSaveQueue({
        localId: localIdRef.current,
        initial,
        debounceMs: AUTOSAVE_MS,
        onStatus: setSaveStatus,
        build: () => ({
          ...idFields.current,
          fromEmail: accountEmail(idFields.current.accountId),
          mode: modeRef.current,
          toJson: recipientRef.current.to,
          ccJson: recipientRef.current.cc,
          bccJson: recipientRef.current.bcc,
          subject: subjectRef.current,
          bodyHtml: editorRef.current?.getHTML() ?? '',
          attachmentsJson: attsRef.current,
        }),
        save: (draft) =>
          api.drafts_upsert({
            draft,
            expectedRevision: queueRef.current?.acknowledgedRevision ?? 0,
          }),
      });
      return queueRef.current;
    },
    [accountEmail],
  );

  const editor = useEditor({
    extensions: [
      StarterKit.configure({ heading: false }),
      Link.configure({ openOnClick: false }),
      Placeholder.configure({ placeholder: 'Write…' }),
      SignatureBlock,
      QuoteBlock,
    ],
    content: '',
    editable: false,
    editorProps: {
      // Plain-text mode never lets pasted markup carry formatting in.
      transformPastedHTML: (html) => (plainRef.current ? escapeHtml(htmlToPlainText(html)) : html),
    },
  });
  editorRef.current = editor;

  /** One revision per edit; the queue debounces and serializes the writes. */
  const markChanged = useCallback(() => {
    startQueue().change();
  }, [startQueue]);

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

  // The body accepts typing only once the account, the stored draft or the
  // quoted message is actually there: a late response can never overwrite it.
  // A queued payload is frozen (P6.2): cancel it first, then edit a new revision.
  const locked = restoring || queued !== null;
  const bodyReady = !locked && quoted !== 'loading';
  readyRef.current = bodyReady;
  useEffect(() => {
    // Making the body editable is not an edit: it must not mark a change.
    editor?.setEditable(bodyReady, false);
  }, [editor, bodyReady]);

  // ---- account selection -------------------------------------------------
  useEffect(() => {
    if (activeMode !== 'new' || from || !accounts.length) return;
    const id = resolveComposeAccount({
      scope: accountScope,
      accounts,
      enabledIds,
      lastSender: readLastSender(),
    });
    if (id) setFrom(id);
  }, [activeMode, from, accounts, accountScope, enabledIds]);

  useEffect(() => {
    idFields.current = { ...idFields.current, accountId: from };
  }, [from]);

  const changeFrom = (id: string) => {
    setFrom(id);
    rememberLastSender(id);
    sigAppliedRef.current = id;
    if (editor && quoted !== 'loading') applySignature(editor, signatureSource(accounts, id));
    markChanged();
  };

  // New messages get their account's signature once; replies get theirs from
  // the prefill. Both paths mark the account so a re-render cannot duplicate
  // the block, and a restored draft keeps exactly the body it was saved with.
  const contentReadyRef = useRef(activeMode === 'new' && !draftId);
  useEffect(() => {
    if (!editor || !from || !accounts.length || sigAppliedRef.current === from) return;
    if (!contentReadyRef.current) return;
    sigAppliedRef.current = from;
    applySignature(editor, signatureSource(accounts, from));
  }, [editor, from, accounts]);

  // ---- reopening a stored draft (P5.2) -----------------------------------
  useEffect(() => {
    if (!draftId) return;
    let cancelled = false;
    api
      .drafts_get({ localId: draftId })
      .then((d) => {
        if (cancelled) return;
        if (!d) {
          throw new Error('draft_missing');
        }
        setTo(d.toJson ?? []);
        setCc(d.ccJson ?? []);
        setBcc(d.bccJson ?? []);
        recipientRef.current = { to: d.toJson ?? [], cc: d.ccJson ?? [], bcc: d.bccJson ?? [] };
        setShowCc((d.ccJson ?? []).length > 0);
        setShowBcc((d.bccJson ?? []).length > 0);
        setSubject(d.subject ?? '');
        subjectRef.current = d.subject ?? '';
        setFrom(d.accountId);
        setActiveMode(d.mode || mode);
        attsRef.current = d.attachmentsJson ?? [];
        setAtts(d.attachmentsJson ?? []);
        idFields.current = {
          accountId: d.accountId,
          threadId: d.threadId,
          inReplyToMessageId: d.inReplyToMessageId,
          rfcMessageId: d.rfcMessageId,
          remoteDraftId: d.remoteDraftId,
          remoteMessageId: d.remoteMessageId,
        };
        sigAppliedRef.current = d.accountId;
        contentReadyRef.current = true;
        // The stored body is applied once the editor exists (below): the editor
        // may not have been created yet when this response arrived.
        setPendingDraft(d);
      })
      .catch(() => {
        if (cancelled) return;
        setRestoring(false);
        setQuoted('error');
        toast.error('Could not open this draft');
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftId]);

  /**
   * The stored body is authoritative, so it is written once both the draft and
   * the editor exist — never with a stale (empty) editor, and never as an edit.
   */
  useEffect(() => {
    if (!editor || !pendingDraft) return;
    editor.commands.setContent(pendingDraft.bodyHtml ?? '', false);
    editor.commands.setTextSelection(1);
    startQueue(pendingDraft);
    setPendingDraft(null);
    setRestoring(false);
  }, [editor, pendingDraft, startQueue]);

  // ---- reply / forward prefill ------------------------------------------
  useEffect(() => {
    if (!editor || draftId || !thread) return;
    if (mode !== 'reply' && mode !== 'reply_all' && mode !== 'forward') return;
    let cancelled = false;
    void (async () => {
      try {
        const detail = await api.thread_get(thread.accountId, thread.threadId);
        if (cancelled) return;
        const p = pickParent(detail.messages, thread.messageId);
        if (!p) {
          setQuoted('ready');
          return;
        }
        setParent(p);
        setFrom(detail.accountId);
        const identities = identitiesOf(detail.accountId);
        if (mode === 'forward') {
          setSubject(forwardSubject(p.subject));
          subjectRef.current = forwardSubject(p.subject);
        } else {
          const subj = replySubject(p.subject);
          setSubject(subj);
          subjectRef.current = subj;
          if (mode === 'reply_all') {
            const built = replyAllRecipients(p, identities);
            setTo(built.to);
            setCc(built.cc);
            setShowCc(built.cc.length > 0);
            // The ref is what send/close read; a prefill that only set state
            // left the reply with no recipients at send time.
            recipientRef.current = { ...recipientRef.current, to: built.to, cc: built.cc };
          } else {
            const built = replyRecipients(p, identities);
            setTo(built);
            recipientRef.current = { ...recipientRef.current, to: built };
          }
        }
        idFields.current = {
          ...idFields.current,
          accountId: detail.accountId,
          // Threading: the parent id is what the backend needs — it derives the
          // RFC Message-ID/References chain from the stored parent itself. The
          // reply's own Message-ID is assigned on first send and preserved by
          // every later edit of the same draft.
          inReplyToMessageId: mode === 'forward' ? undefined : p.id,
        };
        // The quote is the real body, not the list snippet: poll briefly while
        // the local copy is still being fetched, then say so honestly.
        let body: MessageBody | null = null;
        let delay = 150;
        for (let attempt = 0; attempt < 5; attempt += 1) {
          try {
            body = await api.message_body(thread.accountId, p.id);
          } catch {
            body = null;
            break;
          }
          if (cancelled) return;
          if (body.state !== 'loading') break;
          await new Promise((r) => setTimeout(r, delay));
          delay = Math.min(delay * 2, 800);
        }
        if (cancelled) return;
        const raw = signatureSource(accounts, detail.accountId);
        const available = !!body && body.state === 'ready' && !!(body.html || body.text);
        const block = available
          ? mode === 'forward'
            ? forwardHtml(
                { from: p.from, to: p.to, subject: p.subject, date: p.internalDate },
                { html: body?.html, text: body?.text },
              )
            : quoteHtml({ html: body?.html, text: body?.text, from: p.from, date: p.internalDate })
          : '';
        editor.commands.setContent(composedBodyHtml(raw, block), false);
        editor.commands.setTextSelection(1);
        contentReadyRef.current = true;
        sigAppliedRef.current = detail.accountId;
        setQuoted(available ? 'ready' : 'error');
        // Filling in the reply target, recipients, subject and quote is an edit.
        markChanged();
      } catch {
        if (cancelled) return;
        setQuoted('error');
        toast.error('Could not load the message being answered');
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor, mode, thread?.accountId, thread?.threadId, thread?.messageId, draftId]);

  useEffect(() => {
    if (!bodyReady) return;
    const t = setTimeout(() => editor?.commands.focus('start'), 0);
    return () => clearTimeout(t);
  }, [editor, bodyReady]);

  // ---- draft persistence -------------------------------------------------
  useEffect(() => {
    if (!editor) return;
    const h = () => {
      if (!readyRef.current) return;
      markChanged();
    };
    editor.on('update', h);
    return () => {
      editor.off('update', h);
    };
  }, [editor, markChanged]);

  /**
   * Close, Escape and the window close button all funnel here: the newest
   * snapshot is written first, and a storage failure keeps the composer open
   * so nothing typed can disappear (P5.1).
   */
  const close = useCallback(async () => {
    if (closedRef.current) return;
    snapshotRecipientsRef.current(); // a half-typed address is still an edit
    const q = startQueue();
    const saved = await q.flush();
    if (!saved && q.currentStatus === 'error') {
      toast.error("Couldn't save this draft — check storage and retry");
      return;
    }
    closedRef.current = true;
    q.dispose();
    onClose();
  }, [onClose, startQueue]);

  const closeRef = useRef(close);
  closeRef.current = close;

  // Unmount without close (a route change, an error boundary) must still land
  // the pending snapshot, and it must never fire after the sheet is gone.
  useEffect(
    () => () => {
      const q = queueRef.current;
      if (!q) return;
      if (closedRef.current) q.dispose();
      else void q.flush();
    },
    [],
  );

  // A window close must not drop the pending snapshot either.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      try {
        // Platform-specific module (rule exception): `@tauri-apps/api/window`
        // only exists meaningfully inside a Tauri window. Loading it lazily
        // keeps the composer working in a plain browser or the e2e harness,
        // where `getCurrentWindow()` has no window to return.
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        const win = getCurrentWindow();
        const un = await win.onCloseRequested(async (event) => {
          const q = queueRef.current;
          if (!q || q.currentRevision <= q.acknowledgedRevision) return;
          event.preventDefault();
          const saved = await q.flush();
          if (saved) {
            closedRef.current = true;
            q.dispose();
            await win.close();
          } else {
            toast.error("Couldn't save this draft — check storage and retry");
          }
        });
        if (disposed) un();
        else unlisten = un;
      } catch {
        /* Not a Tauri window (tests, browser preview): nothing to hook. */
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // ---- attachments -------------------------------------------------------
  const stage = async (paths: string[]) => {
    if (!paths.length) return;
    const batch = paths.map((p) => ({ path: p, name: basename(p) }));
    const seq = ++stageSeq.current;
    setPending((x) => [...x, ...batch]);
    try {
      const refs = await api.attachments_add_from_paths({
        accountId: idFields.current.accountId,
        draftId: localIdRef.current,
        paths,
      });
      if (stageSeq.current !== seq) return;
      const next = [...attsRef.current, ...refs]; // refresh chips from the command result
      attsRef.current = next;
      setAtts(next);
      if (refs.length < batch.length) {
        toast.error(`Couldn't attach ${batch.length - refs.length} of ${batch.length} files`);
      }
      markChanged();
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
    const next = attsRef.current.filter((_, i) => i !== index);
    attsRef.current = next;
    setAtts(next);
    markChanged();
  };

  /** Copy one attachment of the forwarded message into this draft (P5.4). */
  const includeForwarded = async (meta: AttachmentMeta, include: boolean) => {
    if (!parent) return;
    if (!include) {
      const next = attsRef.current.filter((a) => a.name !== (meta.filename ?? a.name));
      attsRef.current = next;
      setAtts(next);
      markChanged();
      return;
    }
    setForwardBusy((b) => ({ ...b, [meta.id]: 'loading' }));
    try {
      const ref = await api.attachments_stage_from_message({
        accountId: idFields.current.accountId,
        messageId: parent.id,
        attachmentId: meta.id,
        draftId: localIdRef.current,
      });
      const next = [...attsRef.current, ref];
      attsRef.current = next;
      setAtts(next);
      markChanged();
      setForwardBusy((b) => {
        const next2 = { ...b };
        delete next2[meta.id];
        return next2;
      });
    } catch (e) {
      setForwardBusy((b) => ({ ...b, [meta.id]: 'error' }));
      toast.error(`Couldn't copy ${meta.filename ?? 'the attachment'}: ${errorReason(e)}`);
    }
  };

  // ---- recipients --------------------------------------------------------
  const valuesRef = useRef({ to, cc, bcc });
  valuesRef.current = { to, cc, bcc };
  const setters: Record<Field, (v: Address[]) => void> = { to: setTo, cc: setCc, bcc: setBcc };

  /** Every recipient mutation goes through here: state, ref and revision together. */
  const setRecipients = (field: Field, next: Address[]) => {
    recipientRef.current = { ...recipientRef.current, [field]: next };
    setters[field](next);
    markChanged();
  };

  const commitField = (field: Field) => {
    const raw = texts[field];
    if (!raw.trim()) return recipientRef.current[field];
    const parsed = parseAddressList(raw);
    if (!parsed.length) return recipientRef.current[field];
    const merged = mergeRecipients(valuesRef.current[field], parsed);
    setTexts((t) => ({ ...t, [field]: '' }));
    setRecipients(field, merged);
    return merged;
  };

  /**
   * Commit any half-typed address text and report the recipients as they will
   * be sent. Kept in a ref so close and send share exactly one implementation.
   */
  const snapshotRecipients = (): Record<Field, Address[]> => {
    commitField('to');
    commitField('cc');
    commitField('bcc');
    return { ...recipientRef.current };
  };
  const snapshotRecipientsRef = useRef(snapshotRecipients);
  snapshotRecipientsRef.current = snapshotRecipients;

  const setLayer = useCallback((key: string, open: boolean, dismiss: () => void) => {
    if (open) layers.current.set(key, dismiss);
    else layers.current.delete(key);
    setEscapeOwned(layers.current.size > 0);
  }, []);

  useEffect(() => {
    setLayer('link', linkOpen, () => setLinkOpen(false));
  }, [linkOpen, setLayer]);

  // ---- sending -----------------------------------------------------------

  /**
   * Refill the composer from the draft `send_cancel` returned (P6.2). The
   * returned row is authoritative — recipients, subject, body and the staged
   * attachment refs are the ones the frozen request was built from, so what is
   * reopened is byte-for-byte the mail that was about to be sent.
   */
  const applyReturnedDraft = useCallback(
    (d: Draft) => {
      setTo(d.toJson ?? []);
      setCc(d.ccJson ?? []);
      setBcc(d.bccJson ?? []);
      recipientRef.current = { to: d.toJson ?? [], cc: d.ccJson ?? [], bcc: d.bccJson ?? [] };
      setShowCc((d.ccJson ?? []).length > 0);
      setShowBcc((d.bccJson ?? []).length > 0);
      setSubject(d.subject ?? '');
      subjectRef.current = d.subject ?? '';
      setFrom(d.accountId);
      attsRef.current = d.attachmentsJson ?? [];
      setAtts(d.attachmentsJson ?? []);
      idFields.current = {
        accountId: d.accountId,
        threadId: d.threadId,
        inReplyToMessageId: d.inReplyToMessageId,
        rfcMessageId: d.rfcMessageId,
        remoteDraftId: d.remoteDraftId,
        remoteMessageId: d.remoteMessageId,
      };
      sigAppliedRef.current = d.accountId;
      // Restoring is not an edit: the revision stays the one storage already
      // acknowledged, so the next change is a new revision rather than a
      // mutation of the queued payload.
      editorRef.current?.commands.setContent(d.bodyHtml ?? '', false);
      const q = startQueue(d);
      q.adopt(d);
      setSaveStatus('saved');
    },
    [startQueue],
  );

  /**
   * Read the operation's real state. The outbox is the only authority for
   * whether the provider accepted the message; a queue call returning is not.
   */
  const resolveQueued = useCallback(async (opId: number) => {
    try {
      const op = await api.outbox_get({ opId });
      setQueued((q) =>
        q && q.opId === opId ? { ...q, state: op.state, message: op.errorMessage ?? null } : q,
      );
    } catch {
      // A pruned operation is a resolved one; the last observed state stands
      // rather than being upgraded to a claim the composer cannot support.
    }
  }, []);

  const undoQueued = useCallback(async () => {
    const current = queuedRef.current;
    if (!current) return;
    try {
      const draft = await api.send_cancel({ opId: current.opId });
      applyReturnedDraft(draft);
      setQueued(null);
      setSending(false);
      toast('Send cancelled — nothing was sent');
    } catch (e) {
      const err = readSiftError(e);
      // Too late: the state the backend reported is the honest one, so the
      // composer shows it instead of pretending the undo worked.
      const state = err.detail?.state;
      if (err.code === 'send_undo_expired' && state) {
        setQueued((q) => (q ? { ...q, state, message: err.message } : q));
      }
      toast.error(err.message);
    }
  }, [applyReturnedDraft]);

  const send = async (archive = false, schedule?: ScheduleChoice | null) => {
    if (sending || queuedRef.current) return;
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
    const q = startQueue();
    // One successful save first: the queued operation uses the id and revision
    // storage actually acknowledged, never a guess (P5.1).
    const saved = await q.flush();
    if (!saved) {
      setSending(false);
      toast.error("Couldn't save the draft — the message was not queued");
      return;
    }
    rememberLastSender(from);
    // A scheduled send's deadline is the instant the user picked; only an
    // immediate send carries the Undo-send grace delay (P8.1).
    const notBefore = schedule ? schedule.notBefore : Date.now() + settings.undoSendDelay * 1000;
    try {
      // Send-and-archive is a dependent operation: the backend queues the
      // archive label change behind this send and only runs it once the
      // provider accepted the message. Archiving from here would archive the
      // source thread even when the send fails.
      const handle: SendHandle & ScheduleHandleFields = await api.drafts_send({
        localId: saved.localId,
        revision: saved.revision,
        notBefore,
        archiveAfterSend: schedule ? schedule.archiveAfterSend : archive,
      });
      const { opId } = handle;
      setQueued({
        opId,
        notBefore: handle.notBefore || notBefore,
        state: 'pending',
        message: null,
        schedule: schedule
          ? {
              localTime: handle.scheduledLocalTime || schedule.localTime,
              timezone: handle.scheduledTimezone || schedule.timezone,
            }
          : null,
      });
      setSending(false);
      setScheduleOpen(false);
      // The composer stays open on "Queued · Undo": the draft row is still
      // there, and Undo can still reopen it (P6.2).
    } catch (e) {
      const err = readSiftError(e);
      toast.error(err.code === 'draft_queued' ? err.message : 'Send failed. Draft kept');
      setSending(false);
    }
  };
  /** The outbox is the authority: follow the operation until it is resolved. */
  useEffect(() => {
    const opId = queued?.opId;
    if (opId === undefined) return;
    void resolveQueued(opId);
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void on('outbox:state', () => void resolveQueued(opId))
      .then((u) => {
        if (disposed) u();
        else unlisten = u;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queued?.opId, resolveQueued]);

  /**
   * Acceptance is the only event that earns the word "Sent" and the only one
   * that closes the composer (P6.2). Failure keeps it open with the failure on
   * screen, where Retry is one click away.
   */
  useEffect(() => {
    if (queued?.state !== 'done') return;
    toast('Sent');
    if (closedRef.current) return;
    closedRef.current = true;
    queueRef.current?.dispose();
    onClose();
  }, [queued?.state, onClose]);

  const retryQueued = useCallback(async () => {
    const current = queuedRef.current;
    if (!current || current.state !== 'failed') return;
    try {
      await api.outbox_retry({ opId: current.opId, acknowledgeDuplicateRisk: false });
      await resolveQueued(current.opId);
    } catch (e) {
      toast.error(readSiftError(e).message);
    }
  }, [resolveQueued]);

  /**
   * Send a scheduled message right now (P8.1). This clears the deadline; it
   * does not bypass the claim, so the banner keeps following the outbox and
   * never reports "Sent" before the provider accepted it.
   */
  const sendQueuedNow = useCallback(async () => {
    const current = queuedRef.current;
    if (!current) return;
    try {
      await utilities.send_now({ opId: current.opId });
      await resolveQueued(current.opId);
    } catch (e) {
      toast.error(readSiftError(e).message);
    }
  }, [resolveQueued]);

  /**
   * Move an already-queued send's deadline. Past the claim boundary the backend
   * refuses; the refusal is shown as-is rather than the banner pretending the
   * move happened.
   */
  const moveSchedule = useCallback(
    async (choice: ScheduleChoice) => {
      const current = queuedRef.current;
      if (!current) return;
      setMovingSchedule(false);
      try {
        const handle = await utilities.send_reschedule({
          opId: current.opId,
          notBefore: choice.notBefore,
          scheduledLocalTime: choice.localTime,
          scheduledTimezone: choice.timezone,
        });
        setQueued((q) =>
          q && q.opId === current.opId
            ? {
                ...q,
                notBefore: handle.notBefore || choice.notBefore,
                schedule: { localTime: choice.localTime, timezone: choice.timezone },
              }
            : q,
        );
        toast(`Scheduled for ${formatInZone(choice.notBefore, choice.timezone)}`);
      } catch (e) {
        toast.error(readSiftError(e).message);
        await resolveQueued(current.opId);
      }
    },
    [resolveQueued],
  );

  const sendRef = useRef(send);
  sendRef.current = send;
  const pickAttachmentsRef = useRef(pickAttachments);
  pickAttachmentsRef.current = pickAttachments;

  /**
   * One predicate for every send entry point (P8.1): the split control must not
   * offer to schedule a message the primary Send button would refuse.
   */
  const sendBlocked = sending || locked || pending.length > 0 || quoted === 'loading';

  /**
   * The composer dismisses itself (P9.5): its own capture handler owns Escape
   * because a closed sheet has to flush the pending draft first, and any
   * dismissible layer inside it (recipient suggestions, link popover) closes
   * before the sheet does. Reporting itself as `native` keeps the app-level
   * handler from unsubscribing the sheet out from under that flush.
   */
  useEffect(() => pushSurface({ kind: 'native', owner: 'compose' }), []);

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
      if (mod && e.shiftKey && e.key.toLowerCase() === 'l') {
        // Same guarantee as the Send path: the draft is flushed before the
        // scheduling dialog opens, so a schedule can never describe text the
        // draft does not have (P8.1).
        e.preventDefault();
        setScheduleOpen(true);
        return;
      }
      if (plainRef.current && mod && ['b', 'i', 'u'].includes(e.key.toLowerCase())) {
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      if (e.key === 'Escape') {
        // A surface above the composer (a menu or popover opened from it) owns
        // this Escape; only the composer's own layers/sheet react to it (P9.5).
        if (topSurface()?.owner !== 'compose') return;
        // The innermost dismissible layer closes first; the next Escape closes
        // the sheet itself, after the pending draft has been flushed.
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
        void closeRef.current();
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
  const forwardedMeta = (parent?.attachments ?? []).filter((a) => !a.isInline);
  const title = activeMode === 'new' ? 'New message' : activeMode === 'forward' ? 'Forward' : 'Reply';

  return (
    <Sheet onDismiss={() => void close()}>
      <div
        ref={rootRef}
        data-compose-root="1"
        data-compose-escape={escapeOwned ? '1' : undefined}
        data-save-status={saveStatus}
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
            {restoring ? 'Opening draft…' : title}
          </span>
          <SaveStatusLabel
            status={saveStatus}
            onRetry={() => {
              snapshotRecipientsRef.current();
              void startQueue().retry();
            }}
          />
          {accounts.length > 1 && (
            <select
              value={from}
              onChange={(e) => changeFrom(e.target.value)}
              aria-label="From account"
              disabled={locked}
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
            type="button"
            onClick={() => void close()}
            aria-label="Close composer"
            style={iconButtonStyle}
          >
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
              disabled={locked}
              onText={(v) => setTexts((t) => ({ ...t, [field]: v }))}
              onChange={(v) => setRecipients(field, v)}
              onCommit={() => {
                commitField(field);
                markChanged();
              }}
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
          onChange={(e) => {
            setSubject(e.target.value);
            subjectRef.current = e.target.value;
            markChanged();
          }}
          placeholder="Subject"
          aria-label="Subject"
          disabled={locked}
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
            .tiptap details.sift-quote { margin: 8px 0; color: var(--fg-2); }
            .tiptap details.sift-quote summary { cursor: pointer; font-size: 12px; color: var(--fg-3); }
            .tiptap details.sift-quote blockquote { margin: 6px 0 0; }
          `}</style>
          {quoted === 'loading' && (
            <div
              role="status"
              style={{ display: 'flex', gap: 8, alignItems: 'center', fontSize: 12, color: 'var(--fg-3)' }}
            >
              <Spinner size={12} /> Loading the message being answered…
            </div>
          )}
          {quoted === 'error' && (
            <div role="status" style={{ fontSize: 12, color: 'var(--fg-3)' }}>
              The original message could not be loaded — writing without a quote.
            </div>
          )}
          <EditorContent editor={editor} />
        </div>

        {activeMode === 'forward' && forwardedMeta.length > 0 && (
          <div
            aria-label="Original attachments"
            style={{
              borderTop: '1px solid var(--border)',
              padding: '8px 16px',
              display: 'flex',
              flexDirection: 'column',
              gap: 6,
            }}
          >
            <span style={{ fontSize: 12, color: 'var(--fg-3)' }}>Include from the original message</span>
            {forwardedMeta.map((a) => {
              const busy = forwardBusy[a.id];
              const included = atts.some((x) => x.name === (a.filename ?? x.name));
              return (
                <label
                  key={a.id}
                  style={{ display: 'flex', gap: 8, alignItems: 'center', fontSize: 12, color: 'var(--fg)' }}
                >
                  <input
                    type="checkbox"
                    checked={included}
                    disabled={busy === 'loading'}
                    onChange={(e) => void includeForwarded(a, e.target.checked)}
                    aria-label={`Include ${a.filename ?? a.id}`}
                  />
                  <Paperclip size={12} />
                  {a.filename ?? a.id}
                  <span style={{ color: 'var(--fg-3)' }}>{(a.size / 1024).toFixed(0)}KB</span>
                  {busy === 'loading' && <Spinner size={12} />}
                  {busy === 'error' && <span style={{ color: 'var(--danger)' }}>Couldn’t copy</span>}
                </label>
              );
            })}
          </div>
        )}

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

        {queued && (
          <QueuedBanner
            queued={queued}
            onUndo={() => void undoQueued()}
            onRetry={() => void retryQueued()}
            onSendNow={() => void sendQueuedNow()}
            onEditTime={() => setMovingSchedule(true)}
          />
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
          <div style={{ display: 'inline-flex' }}>
            <Button
              variant="primary"
              onClick={() => void send()}
              disabled={sendBlocked}
              style={{
                padding: '8px 20px',
                height: 36,
                fontWeight: 600,
                // The caret beside it is the other half of one control (P8.1).
                borderTopRightRadius: 0,
                borderBottomRightRadius: 0,
              }}
            >
              {sending ? 'Sending…' : 'Send ⌘↵'}
            </Button>
            <SendLaterMenu
              disabled={sendBlocked}
              onSendNow={() => void send()}
              onTomorrow={(at) =>
                void send(false, {
                  notBefore: at.getTime(),
                  localTime: localWallTime(at),
                  timezone: timezoneName(),
                  archiveAfterSend: false,
                })
              }
              onChoose={() => setScheduleOpen(true)}
            />
          </div>
          <Button onClick={() => void send(true)} disabled={sendBlocked} title="Send and archive (⌘⇧↵)">
            Send & archive
          </Button>
          <Button
            onClick={() => void pickAttachments()}
            disabled={sending || locked || pending.length > 0}
            title="Attach (⌘⇧A)"
          >
            Attach
          </Button>
          <span style={{ flex: 1 }} />
          <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>Esc saves draft</span>
        </div>
      </div>
      <ScheduleDialog
        open={scheduleOpen}
        archiveDefault={settings.sendAndArchiveDefault}
        onClose={() => setScheduleOpen(false)}
        onConfirm={(choice) => void send(false, choice)}
      />
      {movingSchedule && queued?.schedule && (
        <ScheduleDialog
          open
          title="Change send time"
          submitLabel="Move send"
          initialLocal={queued.schedule.localTime}
          onClose={() => setMovingSchedule(false)}
          onConfirm={(choice) => void moveSchedule(choice)}
        />
      )}
    </Sheet>
  );
}

/**
 * What the composer says after queueing (P6.2). The word "Sent" is reserved for
 * provider acceptance; every other state is described as what is actually
 * known, including the one case where nobody knows yet.
 */
function QueuedBanner({
  queued,
  onUndo,
  onRetry,
  onSendNow,
  onEditTime,
}: {
  queued: QueuedSend;
  onUndo: () => void;
  onRetry: () => void;
  onSendNow: () => void;
  onEditTime: () => void;
}) {
  const tone =
    queued.state === 'failed'
      ? 'var(--danger)'
      : queued.state === 'uncertain'
        ? 'var(--warning)'
        : 'var(--fg-2)';
  const text: Record<OperationState, string> = {
    pending: queued.schedule ? 'Scheduled' : 'Queued · Undo',
    inflight: 'Sending now — too late to undo',
    uncertain: 'Unconfirmed: Sift cannot prove this was not sent',
    failed: 'Not sent',
    done: 'Sent',
    cancelled: 'Send cancelled',
  };
  return (
    <div
      role="status"
      aria-live="polite"
      data-testid="compose-queued"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        padding: '10px 16px',
        borderTop: '1px solid var(--border)',
        background: 'var(--bg-elevated)',
        fontSize: 12.5,
        color: tone,
      }}
    >
      <span style={{ fontWeight: 550 }}>{text[queued.state]}</span>
      {queued.schedule && queued.state !== 'cancelled' && (
        <span style={{ color: 'var(--fg-2)' }} data-testid="compose-scheduled-at">
          sends {formatInZone(queued.notBefore, queued.schedule.timezone)} · {queued.schedule.timezone}
        </span>
      )}
      {queued.schedule && queued.state === 'pending' && (
        <span style={{ color: 'var(--fg-3)' }}>{SEND_LATER_COPY}</span>
      )}
      {queued.state === 'uncertain' && (
        <span style={{ color: 'var(--fg-3)' }}>the Outbox shows the reconciliation state</span>
      )}
      {queued.state === 'failed' && queued.message && (
        <span style={{ color: 'var(--fg-3)', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>
          {queued.message}
        </span>
      )}
      <span style={{ flex: 1 }} />
      {queued.state === 'pending' && queued.schedule && (
        <>
          <Button onClick={onEditTime} style={{ height: 28, padding: '0 12px' }}>
            Edit time
          </Button>
          <Button onClick={onSendNow} style={{ height: 28, padding: '0 12px' }}>
            Send now
          </Button>
        </>
      )}
      {queued.state === 'pending' && (
        <Button onClick={onUndo} style={{ height: 28, padding: '0 12px' }}>
          Undo
        </Button>
      )}
      {queued.state === 'failed' && (
        <Button onClick={onRetry} style={{ height: 28, padding: '0 12px' }}>
          Retry
        </Button>
      )}
    </div>
  );
}

/**
 * The only draft status the user needs: writing now, written, or a storage
 * failure with a retry — never a claim about connectivity (P5.1).
 */
function SaveStatusLabel({ status, onRetry }: { status: SaveStatus; onRetry: () => void }) {
  if (status === 'saving') {
    return (
      <span role="status" aria-live="polite" style={{ fontSize: 11, color: 'var(--fg-3)' }}>
        Saving…
      </span>
    );
  }
  if (status === 'saved') {
    return (
      <span role="status" aria-live="polite" style={{ fontSize: 11, color: 'var(--fg-3)' }}>
        Saved
      </span>
    );
  }
  if (status === 'error') {
    return (
      <span
        role="status"
        aria-live="assertive"
        title="Saving to local storage failed"
        style={{ fontSize: 11, color: 'var(--danger)', display: 'inline-flex', gap: 6 }}
      >
        Couldn’t save
        <button type="button" onClick={onRetry} style={{ ...linkButtonStyle, color: 'var(--danger)' }}>
          Retry
        </button>
      </span>
    );
  }
  return null;
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
  disabled,
  onText,
  onChange,
  onCommit,
  accountId,
  onLayer,
}: {
  label: string;
  values: Address[];
  text: string;
  disabled?: boolean;
  onText: (v: string) => void;
  onChange: (v: Address[]) => void;
  onCommit: () => void;
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
                      ? COLOR_MIX_SUPPORTED
                        ? 'color-mix(in oklab, var(--danger) 12%, transparent)'
                        : 'var(--n2)'
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
                disabled={disabled}
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
          disabled={disabled}
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
