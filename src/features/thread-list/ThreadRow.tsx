import React, { memo, useState } from 'react';
import type { ThreadRow as Row } from '../../app/ipc/types';
import { Avatar } from '../../ui/Avatar';
import { Chip } from '../../ui/Chip';
import { formatRowDate } from '../../lib/dates';
import { participantsLabel } from '../../lib/names';
import { accentHex } from '../../lib/colors';
import { useSettings } from '../../stores/settingsStore';
import { Archive, Clock, Mail, MailOpen, Star, Paperclip, Trash2 } from 'lucide-react';

export type RowAction = 'archive' | 'trash' | 'read' | 'star' | 'snooze';

interface Props {
  row: Row;
  focused: boolean;
  selected: boolean;
  accountColor?: string;
  showStripe: boolean;
  onFocus: () => void;
  onToggleSelect: (e: React.MouseEvent) => void;
  onOpen: () => void;
  onAction?: (kind: RowAction) => void;
}

function HoverActions({ row, onAction }: { row: Row; onAction?: (kind: RowAction) => void }) {
  if (!onAction) return null;
  const btn: React.CSSProperties = {
    width: 26,
    height: 26,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    background: 'var(--bg-list)',
    border: '1px solid var(--border)',
    borderRadius: 6,
    cursor: 'pointer',
    color: 'var(--fg-2)',
  };
  const stop = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    fn();
  };
  return (
    <span style={{ display: 'inline-flex', gap: 2, flexShrink: 0 }} onMouseEnter={(e) => e.stopPropagation()}>
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

export const ThreadRowView = memo(function ThreadRowView({
  row,
  focused,
  selected,
  accountColor,
  showStripe,
  onFocus,
  onToggleSelect,
  onOpen,
  onAction,
}: Props) {
  const [hover, setHover] = useState(false);
  const avatars = useSettings((s) => s.settings.avatarsInList);
  const density = useSettings((s) => s.settings.density);
  const h = density === 'compact' ? 32 : density === 'comfortable' ? 48 : 40;
  const unread = row.unreadCount > 0;
  const chips = row.labelIds
    .filter((l) => !['INBOX', 'UNREAD', 'STARRED', 'SENT', 'DRAFT', 'SPAM', 'TRASH', 'IMPORTANT'].includes(l))
    .slice(0, 2);
  const overflow =
    row.labelIds.filter(
      (l) => !['INBOX', 'UNREAD', 'STARRED', 'SENT', 'DRAFT', 'SPAM', 'TRASH', 'IMPORTANT'].includes(l),
    ).length - chips.length;

  if (density === 'compact') {
    return (
      <div
        role="option"
        aria-selected={selected}
        onMouseEnter={(e) => {
          onFocus();
          setHover(true);
          void e;
        }}
        onMouseLeave={() => setHover(false)}
        onClick={(e) => {
          onFocus();
          if (e.metaKey || e.ctrlKey) onToggleSelect(e);
          else onOpen();
        }}
        style={{
          height: h,
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          padding: '0 12px 0 10px',
          background: focused ? 'var(--bg-row-focus)' : selected ? 'var(--bg-row-selected)' : 'transparent',
          borderLeft: focused ? '2px solid var(--accent)' : '2px solid transparent',
          cursor: 'default',
          position: 'relative',
        }}
        className="hoverable"
        data-testid={`row-${row.id}`}
      >
        {showStripe && (
          <span
            style={{
              position: 'absolute',
              left: focused ? 2 : 0,
              top: 0,
              bottom: 0,
              width: 2,
              background: accentHex(accountColor ?? 'blue'),
            }}
          />
        )}
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
          {row.subject} <span style={{ color: 'var(--fg-3)', fontWeight: 400 }}>— {row.snippet}</span>
        </span>
        {hover && onAction ? (
          <HoverActions row={row} onAction={onAction} />
        ) : (
          <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-3)', flexShrink: 0 }}>
            {formatRowDate(row.lastMessageAt)}
          </span>
        )}
      </div>
    );
  }

  return (
    <div
      role="option"
      aria-selected={selected}
      onMouseEnter={() => {
        onFocus();
        setHover(true);
      }}
      onMouseLeave={() => setHover(false)}
      onClick={(e) => {
        onFocus();
        if (e.metaKey || e.ctrlKey) onToggleSelect(e);
        else if (e.shiftKey) onToggleSelect(e);
        else onOpen();
      }}
      style={{
        height: h,
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        padding: '4px 12px 4px 10px',
        background: focused ? 'var(--bg-row-focus)' : selected ? 'var(--bg-row-selected)' : 'transparent',
        borderLeft: focused ? '2px solid var(--accent)' : '2px solid transparent',
        cursor: 'default',
        position: 'relative',
      }}
      className="hoverable"
      data-testid={`row-${row.id}`}
    >
      {showStripe && (
        <span
          style={{
            position: 'absolute',
            left: focused ? 2 : 0,
            top: 0,
            bottom: 0,
            width: 2,
            background: accentHex(accountColor ?? 'blue'),
          }}
        />
      )}
      {unread && (
        <span
          style={{ width: 6, height: 6, borderRadius: '50%', background: 'var(--unread-dot)', flexShrink: 0 }}
        />
      )}
      {avatars && <Avatar email={row.participants[0]?.e ?? '?'} name={row.participants[0]?.n} size={24} />}
      <div style={{ flex: 1, minWidth: 0 }}>
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
          {hover && onAction ? (
            <HoverActions row={row} onAction={onAction} />
          ) : (
            <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-3)', flexShrink: 0 }}>
              {formatRowDate(row.lastMessageAt)}
            </span>
          )}
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
          <span
            style={{
              fontWeight: unread ? 600 : 400,
              fontSize: 13,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {row.subject}
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
            {row.snippet}
          </span>
          {row.isStarred && <Star size={14} fill="var(--star)" color="var(--star)" />}
          {row.hasAttachments && <Paperclip size={14} color="var(--fg-3)" />}
          {chips.map((c) => (
            <Chip key={c} label={c} />
          ))}
          {overflow > 0 && <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>+{overflow}</span>}
        </div>
      </div>
    </div>
  );
});
