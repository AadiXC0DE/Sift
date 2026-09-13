import React, { useState } from 'react';
import { Dialog } from '../../ui/Dialog';
import { api } from '../../app/ipc/commands';
import type { AttachmentMeta } from '../../app/ipc/types';
import { ExternalLink } from 'lucide-react';

/**
 * Attachment preview (P2.6).
 *
 * Renders only what an explicit MIME allowlist names, and only through the
 * system WebView: an image goes into an `<img>`, a PDF into a frame the
 * WebView's own PDF viewer owns. No document renderer is bundled, and nothing
 * from the file is inserted into the app document, so an executable `.html`
 * part can never become trusted content.
 *
 * The bytes come from the same account-qualified address Save As copies from
 * (`sift-att://<account>/<message>/<attachment>`): the native scheme handler
 * resolves the part under the same ownership check, so a preview request can
 * never reach another account's attachment, and a part that cannot be verified
 * is reported rather than shown.
 */
export function AttachmentPreview({
  accountId,
  messageId,
  attachment,
  name,
  onClose,
  onOpenInApp,
}: {
  accountId: string;
  messageId: string;
  attachment: AttachmentMeta;
  /** The name the file is written under. */
  name: string;
  onClose: () => void;
  onOpenInApp: () => void;
}) {
  const kind = previewKind(attachment.mime);
  const [failed, setFailed] = useState(false);
  const url = attachmentUrl(accountId, messageId, attachment.id);

  return (
    <Dialog open onClose={onClose} title={name} width={720}>
      <div
        data-testid="attachment-preview"
        data-attachment-id={attachment.id}
        style={{
          background: 'var(--n1)',
          border: '1px solid var(--border)',
          borderRadius: 'var(--r-md)',
          minHeight: 200,
          maxHeight: '68vh',
          overflow: 'auto',
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 12,
          padding: kind === null || failed ? 24 : 0,
        }}
      >
        {kind === 'image' && (
          <img
            src={url}
            alt={name}
            data-testid="attachment-preview-image"
            onError={() => setFailed(true)}
            style={{ maxWidth: '100%', maxHeight: '66vh', display: 'block' }}
          />
        )}
        {kind === 'pdf' && (
          <iframe
            src={url}
            title={`Preview of ${name}`}
            data-testid="attachment-preview-frame"
            // No `sandbox`: the WebView's PDF viewer is a plugin, and an opaque
            // or script-less frame stops it rendering. Nothing from the file is
            // placed in the app document either way.
            style={{ width: '100%', height: '64vh', border: 'none', background: 'var(--n0)' }}
          />
        )}
        {kind === null && (
          <div style={notice} data-testid="attachment-preview-unavailable">
            No preview for this file type.
            <div style={noticeSub}>
              Open it with the app your Mac uses for {attachment.mime || 'this file'}.
            </div>
          </div>
        )}
        {kind !== null && failed && (
          <div style={notice} data-testid="attachment-preview-unavailable">
            This attachment could not be loaded.
            <div style={noticeSub}>
              Open it with the app your Mac uses for {attachment.mime || 'this file'}.
            </div>
          </div>
        )}
      </div>
      <div style={{ display: 'flex', gap: 8, marginTop: 12, alignItems: 'center' }}>
        <span style={{ flex: 1, fontSize: 12, color: 'var(--fg-3)' }}>
          {attachment.mime || 'unknown type'} · {formatSize(attachment.size)}
        </span>
        <button onClick={onOpenInApp} className="sift-chip-btn" data-testid="attachment-preview-open">
          <ExternalLink size={13} style={{ marginRight: 4 }} /> Open in default app
        </button>
        <button onClick={onClose} className="sift-chip-btn" data-testid="attachment-preview-close">
          Close
        </button>
      </div>
    </Dialog>
  );
}

/** Types the system WebView can render itself. Everything else is offered to
 *  the default app instead of being guessed at (P2.6). */
export function previewKind(mime: string): 'image' | 'pdf' | null {
  const value = mime.split(';')[0].trim().toLowerCase();
  if (value === 'application/pdf') return 'pdf';
  if (value.startsWith('image/')) return 'image';
  return null;
}

/** The one account-qualified address for an attachment's bytes (P4.2). */
export function attachmentUrl(accountId: string, messageId: string, attachmentId: string): string {
  return `sift-att://${accountId}/${messageId}/${attachmentId}`;
}

const notice: React.CSSProperties = { textAlign: 'center', fontSize: 13, color: 'var(--fg-2)' };
const noticeSub: React.CSSProperties = { marginTop: 4, fontSize: 12, color: 'var(--fg-3)' };

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
