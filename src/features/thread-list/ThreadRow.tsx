import React, { memo } from 'react';
import type { ThreadRow as Row } from '../../app/ipc/types';
import { Avatar } from '../../ui/Avatar';
import { Chip } from '../../ui/Chip';
import { formatRowDate } from '../../lib/dates';
import { participantsLabel } from '../../lib/names';
import { accentHex } from '../../lib/colors';
import { useSettings } from '../../stores/settingsStore';
import { Star, Paperclip } from 'lucide-react';

interface Props {
  row: Row;
  focused: boolean;
  selected: boolean;
  accountColor?: string;
  showStripe: boolean;
  onFocus: () => void;
  onToggleSelect: (e: React.MouseEvent) => void;
  onOpen: () => void;
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
}: Props) {
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
        onMouseEnter={onFocus}
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
              left: 0,
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
        <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-3)', flexShrink: 0 }}>
          {formatRowDate(row.lastMessageAt)}
        </span>
      </div>
    );
  }

  return (
    <div
      role="option"
      aria-selected={selected}
      onMouseEnter={onFocus}
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
            left: 0,
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
          <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-3)', flexShrink: 0 }}>
            {formatRowDate(row.lastMessageAt)}
          </span>
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
