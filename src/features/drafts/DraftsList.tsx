import React, { useMemo } from 'react';
import type { Draft } from '../../app/ipc/types';
import { useAccounts } from '../../stores/accountsStore';
import { formatAddressList } from '../compose/recipients';
import { htmlToPlainText } from '../compose/plainText';
import { relativeTime } from '../../lib/dates';
import { EmptyState } from '../../ui/EmptyState';
import { Spinner } from '../../ui/Spinner';
import { Chip } from '../../ui/Chip';
import { FileText, Paperclip } from 'lucide-react';
import { useDrafts } from './useDrafts';

// Only non-`editing` states carry a chip; `editing` is the default and says
// nothing. Unknown states fall back to their raw name rather than vanishing.
const STATE_LABEL: Record<string, string> = {
  queued: 'Queued',
  sent: 'Sent',
  failed: 'Failed',
};
const STATE_COLOR: Record<string, string> = {
  queued: 'var(--warning)',
  sent: 'var(--success)',
  failed: 'var(--danger)',
};

const ROW: React.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  gap: 3,
  width: '100%',
  padding: '9px 12px',
  border: 'none',
  borderBottom: '1px solid var(--border)',
  background: 'transparent',
  color: 'inherit',
  font: 'inherit',
  textAlign: 'left',
  cursor: 'pointer',
};

const LINE1: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 8,
  fontSize: 12,
  color: 'var(--fg-2)',
  minWidth: 0,
};

const ELLIPSIS: React.CSSProperties = {
  whiteSpace: 'nowrap',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
};

/**
 * Recipient summary. `To:` leads when present; otherwise the first non-empty
 * of Cc/Bcc names the audience so a draft with only a copy list is not blank.
 * More than three addresses in total add a `+N more` count.
 */
function recipientLine(draft: Draft): { text: string | null; more: number } {
  const { toJson, ccJson, bccJson } = draft;
  const total = toJson.length + ccJson.length + bccJson.length;
  const more = total > 3 ? total - 3 : 0;
  if (toJson.length) return { text: `To: ${formatAddressList(toJson)}`, more };
  if (ccJson.length) return { text: `Cc: ${formatAddressList(ccJson)}`, more };
  if (bccJson.length) return { text: `Bcc: ${formatAddressList(bccJson)}`, more };
  return { text: null, more: 0 };
}

function DraftRow({
  draft,
  accountEmail,
  onOpen,
}: {
  draft: Draft;
  accountEmail: string;
  onOpen: (d: Draft) => void;
}) {
  const subject = draft.subject.trim() || '(No subject)';
  const recipients = recipientLine(draft);
  // Body preview: markup flattened, whitespace collapsed, capped for one line.
  const snippet = htmlToPlainText(draft.bodyHtml).replace(/\s+/g, ' ').trim().slice(0, 140);
  const attachments = draft.attachmentsJson.length;
  const chipLabel = draft.state === 'editing' ? null : (STATE_LABEL[draft.state] ?? draft.state);
  const chipColor = STATE_COLOR[draft.state] ?? 'var(--accent)';

  return (
    <button
      type="button"
      data-testid="draft-row"
      aria-label={`Open draft: ${subject}`}
      onClick={() => onOpen(draft)}
      style={ROW}
    >
      <div style={LINE1}>
        <span style={{ ...ELLIPSIS, flex: 1, minWidth: 0 }}>{accountEmail}</span>
        {attachments > 0 && (
          <span
            aria-label={`${attachments} attachments`}
            style={{ display: 'inline-flex', alignItems: 'center', gap: 3, flexShrink: 0 }}
          >
            <Paperclip size={12} />
            {attachments}
          </span>
        )}
        {chipLabel && <Chip label={chipLabel} color={chipColor} />}
        {typeof draft.updatedAt === 'number' && draft.updatedAt > 0 && (
          <span style={{ flexShrink: 0, fontVariantNumeric: 'tabular-nums' }}>
            {relativeTime(draft.updatedAt)}
          </span>
        )}
      </div>
      {recipients.text && (
        <div style={{ ...LINE1, color: 'var(--fg-3)' }}>
          <span style={{ ...ELLIPSIS, minWidth: 0 }}>{recipients.text}</span>
          {recipients.more > 0 && <span style={{ flexShrink: 0 }}>{`+${recipients.more} more`}</span>}
        </div>
      )}
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 8, minWidth: 0 }}>
        <span style={{ ...ELLIPSIS, fontSize: 13, fontWeight: 600, color: 'var(--fg)', maxWidth: '55%' }}>
          {subject}
        </span>
        {snippet && (
          <span style={{ ...ELLIPSIS, flex: 1, minWidth: 0, fontSize: 12, color: 'var(--fg-3)' }}>
            {snippet}
          </span>
        )}
      </div>
    </button>
  );
}

export function DraftsList({
  accountIds,
  onOpenDraft,
}: {
  accountIds: string[];
  onOpenDraft: (d: Draft) => void;
}) {
  const { rows, nextCursor, initialLoading, loadingMore, error, loadMore, reload } = useDrafts(accountIds);

  const accounts = useAccounts((s) => s.accounts);
  const emailByAccount = useMemo(() => {
    const map = new Map<string, string>();
    for (const a of accounts) map.set(a.id, a.email);
    return map;
  }, [accounts]);

  const body = initialLoading ? (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        padding: '10px 12px',
        fontSize: 12.5,
        color: 'var(--fg-3)',
      }}
    >
      <Spinner />
      Loading drafts
    </div>
  ) : rows.length === 0 ? (
    error ? null : (
      <EmptyState
        icon={<FileText size={24} />}
        line="No drafts"
        sub="Messages you compose are saved here automatically."
      />
    )
  ) : (
    <>
      <div role="list" style={{ display: 'flex', flexDirection: 'column' }}>
        {rows.map((draft) => (
          <DraftRow
            key={`${draft.accountId}:${draft.localId}`}
            draft={draft}
            accountEmail={emailByAccount.get(draft.accountId) ?? draft.fromEmail ?? draft.accountId}
            onOpen={onOpenDraft}
          />
        ))}
      </div>
      {nextCursor && (
        <div style={{ display: 'flex', justifyContent: 'center', padding: 8 }}>
          <button
            type="button"
            className="sift-chip-btn"
            onClick={loadMore}
            disabled={loadingMore}
            style={{ minWidth: 120 }}
          >
            {loadingMore && <Spinner size={12} />}
            Load more
          </button>
        </div>
      )}
    </>
  );

  return (
    <div
      style={{
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        overflowY: 'auto',
      }}
    >
      {error && (
        <div
          role="alert"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            padding: '8px 12px',
            fontSize: 12.5,
            color: 'var(--fg-2)',
            borderBottom: '1px solid var(--border)',
          }}
        >
          <span style={{ flex: 1, minWidth: 0 }}>{error}</span>
          <button type="button" className="sift-chip-btn" onClick={reload}>
            Retry
          </button>
        </div>
      )}
      {body}
    </div>
  );
}
