import React, { useCallback, useRef, useState } from 'react';
import { api } from '../../app/ipc/commands';
import type { AttachmentMeta, AttachmentRefKey } from '../../app/ipc/types';
import { Spinner } from '../../ui/Spinner';
import { Paperclip, Download } from 'lucide-react';
import { toast } from 'sonner';
import { attachmentUrl, AttachmentPreview } from './AttachmentPreview';

/**
 * The attachments of one message.
 *
 * Two membership rules matter (P2.6):
 *
 * * a part without a filename is still an attachment. The strip used to drop
 *   every unnamed part, which hid real files from the reader and from Save All;
 * * an inline image belongs in the email body unless the reader explicitly asks
 *   for it, so inline parts stay behind the "All attachments" toggle.
 *
 * Save All appears once a message has two or more non-inline attachments, and
 * Space on a focused attachment opens the preview sheet.
 */
export function AttachmentStrip({
  accountId,
  messageId,
  attachments,
}: {
  accountId: string;
  messageId: string;
  attachments: AttachmentMeta[];
}) {
  const strip = useRef<HTMLDivElement>(null);
  const [showInline, setShowInline] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [preview, setPreview] = useState<AttachmentMeta | null>(null);
  const [savingAll, setSavingAll] = useState(false);
  const [saveAll, setSaveAll] = useState<{ saved: number; failed: AttachmentRefKey[] } | null>(null);

  const nonInline = attachments.filter((a) => !a.isInline);
  const visible = showInline ? attachments : nonInline;
  const hiddenInline = attachments.length - nonInline.length;

  const focusBack = useCallback((id: string, action: 'open' | 'save') => {
    const active = document.activeElement;
    // Never steal focus from somewhere the reader has since moved it.
    if (active && active !== document.body && !strip.current?.contains(active)) return;
    strip.current
      ?.querySelector<HTMLElement>(`[data-attachment-focus="${action}-${id}"]`)
      ?.focus({ preventScroll: true });
  }, []);

  const errorText = (e: unknown): string => {
    if (typeof e === 'string') return e;
    if (
      e &&
      typeof e === 'object' &&
      'message' in e &&
      typeof e.message === 'string' &&
      e.message.length < 200
    ) {
      return e.message;
    }
    return 'Could not fetch this attachment. Check your connection and try again.';
  };

  const open = async (a: AttachmentMeta) => {
    setBusy(a.id);
    try {
      await api.attachments_open({ accountId, attachmentId: a.id });
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(null);
      focusBack(a.id, 'open');
    }
  };

  const save = async (a: AttachmentMeta) => {
    setBusy(a.id);
    try {
      const r = await api.attachments_save_as({ accountId, attachmentId: a.id });
      if (r?.path) toast.success(`Saved ${attachmentName(a)}`);
    } catch (e) {
      // A cancelled save dialog is not an error.
      const text = errorText(e);
      if (!/cancel/i.test(text)) toast.error(text);
    } finally {
      setBusy(null);
      focusBack(a.id, 'save');
    }
  };

  const saveEvery = async () => {
    setSavingAll(true);
    try {
      const r = await api.attachments_save_all({ accountId, messageId });
      // A dismissed folder prompt saves nothing and reports nothing.
      if (r.saved === 0 && r.failed.length === 0) return;
      setSaveAll(r);
      if (r.failed.length === 0) toast.success(`Saved ${r.saved} attachments`);
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setSavingAll(false);
    }
  };

  if (!attachments.length) return null;

  const failedNames = (saveAll?.failed ?? []).map((key) => {
    const match = attachments.find((a) => a.id === key.attachmentId);
    return match ? attachmentName(match) : key.attachmentId;
  });

  return (
    <div ref={strip} style={{ margin: '8px 0' }} data-testid="attachment-strip">
      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
        {visible.map((a) => (
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
              onClick={() => void open(a)}
              onKeyDown={(e) => {
                // Space previews; Enter/click still hands the file to the
                // default app. Only the open control claims Space.
                if (e.key !== ' ') return;
                e.preventDefault();
                setPreview(a);
              }}
              aria-busy={busy === a.id}
              data-attachment-id={a.id}
              data-attachment-focus={`open-${a.id}`}
              aria-keyshortcuts="Space"
              title={`Open ${attachmentName(a)}`}
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
              {busy === a.id ? (
                <Spinner size={16} />
              ) : a.mime.startsWith('image/') ? (
                <img
                  src={attachmentUrl(accountId, messageId, a.id)}
                  alt=""
                  style={{ width: 32, height: 32, objectFit: 'cover', borderRadius: 4 }}
                />
              ) : (
                <Paperclip size={16} />
              )}
              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                {attachmentName(a)}{' '}
                <span style={{ color: 'var(--fg-3)' }}>{(a.size / 1024).toFixed(0)}KB</span>
              </span>
            </button>
            <button
              onClick={() => void save(a)}
              aria-busy={busy === a.id}
              data-attachment-id={a.id}
              data-attachment-focus={`save-${a.id}`}
              title="Save to disk"
              aria-label={`Save ${attachmentName(a)}`}
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
        {hiddenInline > 0 && (
          <button
            onClick={() => setShowInline((v) => !v)}
            aria-pressed={showInline}
            data-testid="attachment-show-all"
            className="sift-chip-btn"
            title={
              showInline
                ? 'Hide the images shown in the message'
                : 'Also list the images shown in the message'
            }
          >
            All attachments ({attachments.length})
          </button>
        )}
        {nonInline.length >= 2 && (
          <button
            onClick={() => void saveEvery()}
            aria-busy={savingAll}
            data-testid="attachment-save-all"
            className="sift-chip-btn"
            title="Save every attachment to one folder"
          >
            Save all…
          </button>
        )}
      </div>
      {saveAll && saveAll.failed.length > 0 && (
        <div
          role="status"
          data-testid="attachment-save-all-result"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            marginTop: 6,
            fontSize: 12,
            color: 'var(--fg-2)',
          }}
        >
          <span style={{ minWidth: 0 }}>
            Saved {saveAll.saved} of {saveAll.saved + saveAll.failed.length}. {failedNames.join(', ')}{' '}
            {failedNames.length === 1 ? 'needs' : 'need'} another try.
          </span>
          <button className="sift-chip-btn" onClick={() => void saveEvery()}>
            Try again
          </button>
        </div>
      )}
      {preview && (
        <AttachmentPreview
          accountId={accountId}
          messageId={messageId}
          attachment={preview}
          name={attachmentName(preview)}
          onClose={() => {
            const id = preview.id;
            setPreview(null);
            focusBack(id, 'open');
          }}
          onOpenInApp={() => void open(preview)}
        />
      )}
    </div>
  );
}

/**
 * The name the file will be written under: the sender's original, or the same
 * fallback basename the backend derives for a part that arrived without one
 * (P2.6). An unnamed part keeps its place in the strip and in Save All — only
 * the label is derived.
 */
export function attachmentName(a: Pick<AttachmentMeta, 'filename' | 'mime' | 'id'>): string {
  const named = a.filename?.trim();
  if (named) return named;
  const short = a.id.replace(/[^A-Za-z0-9]/g, '').slice(0, 8) || '0';
  return `attachment-${short}.${KNOWN_EXTENSIONS[a.mime.split(';')[0].trim().toLowerCase()] ?? 'bin'}`;
}

/** Mirrors `attachments::naming::known_extension` for the common types. */
const KNOWN_EXTENSIONS: Record<string, string> = {
  'application/pdf': 'pdf',
  'application/zip': 'zip',
  'application/gzip': 'gz',
  'application/x-gzip': 'gz',
  'application/x-tar': 'tar',
  'application/json': 'json',
  'application/xml': 'xml',
  'text/xml': 'xml',
  'text/plain': 'txt',
  'text/csv': 'csv',
  'text/html': 'html',
  'message/rfc822': 'eml',
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/heic': 'heic',
  'image/tiff': 'tiff',
  'image/bmp': 'bmp',
  'image/svg+xml': 'svg',
};
