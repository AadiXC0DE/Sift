import React, { memo, useState } from 'react';
import type { ThreadRow as Row } from '../../app/ipc/types';
import { Avatar } from '../../ui/Avatar';
import { Chip } from '../../ui/Chip';
import { formatRowDate } from '../../lib/dates';
import { participantsLabel } from '../../lib/names';
import { accentHex } from '../../lib/colors';
import { useSettings } from '../../stores/settingsStore';
import { decodeRfc2047 } from '../../lib/rfc2047';
import { rowHeightForDensity } from './rowHeight';
import { Archive, Clock, Mail, MailOpen, Star, Paperclip, Trash2 } from 'lucide-react';

export type RowAction = 'archive' | 'trash' | 'read' | 'star' | 'snooze';

interface Props {
  row: Row;
  focused: boolean;
  selected: boolean;
  accountColor?: string;
  accountLabel?: string;
  showStripe: boolean;
  onFocus: () => void;
  onToggleSelect: (e: React.MouseEvent) => void;
  onOpen: () => void;
  onAction?: (kind: RowAction) => void;
}

// Inset accent dash that identifies the account. It never touches the row
// edges, so same-account rows can never join into a continuous line, and it is
// decorative: the account name travels in the row's accessible label instead.
const ACCOUNT_DASH: React.CSSProperties = {
  position: 'absolute',
  left: 3,
  top: '50%',
  transform: 'translateY(-50%)',
  width: 3,
  height: 12,
  borderRadius: 999,
  pointerEvents: 'none',
};

const SR_ONLY: React.CSSProperties = {
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

function HoverActions({
  row,
  onAction,
  surface,
}: {
  row: Row;
  onAction?: (kind: RowAction) => void;
  surface: string;
}) {
  if (!onAction) return null;
  const btn: React.CSSProperties = {
    width: 26,
    height: 26,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    background: surface,
    border: '1px solid var(--border)',
    borderRadius: 6,
    cursor: 'pointer',
    color: 'var(--fg)',
    flexShrink: 0,
  };
  const stop = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    fn();
  };
  return (
    <span
      style={{
        position: 'absolute',
        right: 8,
        top: 0,
        bottom: 0,
        display: 'inline-flex',
        alignItems: 'center',
        gap: 2,
        paddingLeft: 16,
        background: `linear-gradient(to right, transparent, ${surface} 12px)`,
        zIndex: 1,
      }}
    >
      <button title="Archive (e)" style={btn} onClick={stop(() => onAction('archive'))}>
        <Archive size={14} />
      </button>
      <button title="Trash (#)" style={btn} onClick={stop(() => onAction('trash'))}>
        <Trash2 size={14} />
      </button>
      <button
        title={row.unreadCount > 0 ? 'Mark read' : 'Mark unread'}
        style={btn}
        onClick={stop(() => onAction('read'))}
      >
        {row.unreadCount > 0 ? <MailOpen size={14} /> : <Mail size={14} />}
      </button>
      <button title="Snooze (h)" style={btn} onClick={stop(() => onAction('snooze'))}>
        <Clock size={14} />
      </button>
    </span>
  );
}

function rowSurface(focused: boolean, selected: boolean, hover: boolean): string {
  if (focused) return 'var(--bg-row-focus)';
  if (selected) return 'var(--bg-row-selected)';
  if (hover) return 'var(--bg-row-hover)';
  return 'transparent';
}

export const ThreadRowView = memo(function ThreadRowView({
  row,
  focused,
  selected,
  accountColor,
  accountLabel,
  showStripe,
  onFocus,
  onToggleSelect,
  onOpen,
  onAction,
}: Props) {
  const [hover, setHover] = useState(false);
  const avatars = useSettings((s) => s.settings.avatarsInList);
  const density = useSettings((s) => s.settings.density);
  const h = rowHeightForDensity(density);
  const unread = row.unreadCount > 0;
  const chips = row.labelIds
    .filter((l) => !['INBOX', 'UNREAD', 'STARRED', 'SENT', 'DRAFT', 'SPAM', 'TRASH', 'IMPORTANT'].includes(l))
    .slice(0, 2);
  const overflow =
    row.labelIds.filter(
      (l) => !['INBOX', 'UNREAD', 'STARRED', 'SENT', 'DRAFT', 'SPAM', 'TRASH', 'IMPORTANT'].includes(l),
    ).length - chips.length;
  const surface = rowSurface(focused, selected, hover);
  const shell: React.CSSProperties = {
    height: h,
    maxHeight: h,
    overflow: 'hidden',
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    padding: density === 'compact' ? '0 12px 0 10px' : '4px 12px 4px 10px',
    background: surface,
    borderLeft: focused ? '2px solid var(--accent)' : '2px solid transparent',
    cursor: 'default',
    position: 'relative',
    boxSizing: 'border-box',
    transition: 'background-color 80ms ease',
  };

  const onClick = (e: React.MouseEvent) => {
    // Modifier clicks keep the current anchor: resolve the toggle before the
    // focus moves to the clicked row.
    if (e.metaKey || e.ctrlKey || e.shiftKey) {
      onToggleSelect(e);
      onFocus();
      return;
    }
    onFocus();
    onOpen();
  };

  if (density === 'compact') {
    return (
      <div
        role="option"
        aria-selected={selected}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
        onClick={onClick}
        style={shell}
        className="sift-row"
        data-testid={`row-${row.id}`}
        title={showStripe && accountLabel ? accountLabel : undefined}
      >
        {showStripe && (
          <span
            aria-hidden
            data-testid="account-marker"
            style={{ ...ACCOUNT_DASH, background: accentHex(accountColor ?? 'blue'), opacity: 0.85 }}
          />
        )}
        {showStripe && accountLabel ? <span style={SR_ONLY}>Account: {accountLabel}</span> : null}
        {unread && (
          <span
            style={{
              width: 6,
              height: 6,
              borderRadius: '50%',
              background: 'var(--unread-dot)',
              flexShrink: 0,
            }}
          />
        )}
        <span
          style={{
            fontWeight: unread ? 600 : 400,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            fontSize: 12.5,
          }}
        >
          {participantsLabel(row.participants, row.messageCount)}
        </span>
        <span
          style={{
            fontWeight: unread ? 600 : 400,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            flex: 1,
            fontSize: 12.5,
          }}
        >
          {decodeRfc2047(row.subject)}{' '}
          <span style={{ color: 'var(--fg-3)', fontWeight: 400 }}>· {decodeRfc2047(row.snippet)}</span>
        </span>
        <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-3)', flexShrink: 0 }}>
          {formatRowDate(row.lastMessageAt)}
        </span>
        {hover && onAction ? <HoverActions row={row} onAction={onAction} surface={surface} /> : null}
      </div>
    );
  }

  return (
    <div
      role="option"
      aria-selected={selected}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onClick={onClick}
      style={shell}
      className="sift-row"
      data-testid={`row-${row.id}`}
      title={showStripe && accountLabel ? accountLabel : undefined}
    >
      {showStripe && (
        <span
          aria-hidden
          data-testid="account-marker"
          style={{ ...ACCOUNT_DASH, background: accentHex(accountColor ?? 'blue'), opacity: 0.85 }}
        />
      )}
      {showStripe && accountLabel ? <span style={SR_ONLY}>Account: {accountLabel}</span> : null}
      {unread && (
        <span
          style={{ width: 6, height: 6, borderRadius: '50%', background: 'var(--unread-dot)', flexShrink: 0 }}
        />
      )}
      {avatars && <Avatar email={row.participants[0]?.e ?? '?'} name={row.participants[0]?.n} size={24} />}
      <div style={{ flex: 1, minWidth: 0, overflow: 'hidden' }}>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 8 }}>
          <span
            style={{
              fontWeight: unread ? 600 : 400,
              fontSize: 13,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              flex: 1,
            }}
          >
            {participantsLabel(row.participants, row.messageCount)}
          </span>
          <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-3)', flexShrink: 0 }}>
            {formatRowDate(row.lastMessageAt)}
          </span>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 }}>
          <span
            style={{
              fontWeight: unread ? 600 : 400,
              fontSize: 13,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {decodeRfc2047(row.subject)}
          </span>
          <span
            style={{
              color: 'var(--fg-3)',
              fontSize: 13,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              flex: 1,
            }}
          >
            {decodeRfc2047(row.snippet)}
          </span>
          {row.isStarred && (
            <Star
              size={14}
              fill="var(--star)"
              color="var(--star)"
              style={{ flexShrink: 0, width: 14, height: 14 }}
            />
          )}
          {row.hasAttachments && (
            <Paperclip size={14} color="var(--fg-2)" style={{ flexShrink: 0, width: 14, height: 14 }} />
          )}
          {chips.map((c) => (
            <Chip key={c} label={c} />
          ))}
          {overflow > 0 && <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>+{overflow}</span>}
        </div>
      </div>
      {hover && onAction ? <HoverActions row={row} onAction={onAction} surface={surface} /> : null}
    </div>
  );
});
