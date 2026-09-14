import React, { memo } from 'react';
import type { ThreadRow as Row } from '../../app/ipc/types';
import { Avatar } from '../../ui/Avatar';
import { Chip } from '../../ui/Chip';
import { formatRowDate } from '../../lib/dates';
import { participantsLabel, recipientsLabel } from '../../lib/names';
import { labelKey, useLabels } from '../../stores/labelsStore';
import { accentHex } from '../../lib/colors';
import { useSettings } from '../../stores/settingsStore';
import { decodeRfc2047 } from '../../lib/rfc2047';
import { rowHeightForDensity } from './rowHeight';
import { rowKey } from './threadWindow';
import { Archive, BellOff, Clock, Mail, MailOpen, Star, Paperclip, Trash2 } from 'lucide-react';

export type RowAction = 'archive' | 'trash' | 'read' | 'star' | 'snooze' | 'unsnooze';

/**
 * DOM id for a row (P9.5). Provider thread ids are only unique per account, so
 * the id the listbox points at with `aria-activedescendant` has to carry both.
 */
export function rowDomId(row: Pick<Row, 'accountId' | 'id'>): string {
  return `listrow-${row.accountId}-${row.id}`;
}

interface Props {
  row: Row;
  focused: boolean;
  selected: boolean;
  /** The pointer is over this row (tracked by the list, so the row actions can
   * be rendered outside the listbox; see RowActions). */
  hovered: boolean;
  accountColor?: string;
  accountLabel?: string;
  showStripe: boolean;
  /** Position in the listbox and the size of the list, when known (P9.5). */
  posInSet?: number;
  setSize?: number;
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
  width: 2,
  height: 8,
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

/**
 * The row's pointer actions (archive/trash/read/snooze).
 *
 * They are rendered by the *list*, not by the row, for one reason (P9.5): a
 * focusable control inside a listbox option trips `nested-interactive`, and a
 * focusable control beside the option but inside the listbox trips
 * `aria-required-children`. Both are axe serious/critical. The list therefore
 * draws them as an overlay layer that is a sibling of the listbox, aligned with
 * the hovered or keyboard-active row.
 */
export function RowActions({
  row,
  onAction,
  surface,
  focused,
}: {
  row: Row;
  onAction: (kind: RowAction) => void;
  surface: string;
  focused: boolean;
}) {
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
  // Only the active row's actions are in the tab order (P9.5). The list itself
  // stays a single tab stop; pressing Tab from it reaches the actions of the
  // row the keyboard is on, instead of walkable buttons on every rendered row.
  const tabIndex = focused ? 0 : -1;
  const stop = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    fn();
  };
  const readLabel = row.unreadCount > 0 ? 'Mark read' : 'Mark unread';
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 2,
        paddingLeft: 16,
        background: `linear-gradient(to right, transparent, ${surface} 12px)`,
      }}
    >
      {/* Row actions are shortcuts' pointer equivalents, so every one carries an
          accessible name (the row itself is a single `option`, not a button). */}
      <button
        type="button"
        aria-label="Archive"
        title="Archive (e)"
        tabIndex={tabIndex}
        style={btn}
        onClick={stop(() => onAction('archive'))}
      >
        <Archive size={14} />
      </button>
      <button
        type="button"
        aria-label="Trash"
        title="Trash (#)"
        tabIndex={tabIndex}
        style={btn}
        onClick={stop(() => onAction('trash'))}
      >
        <Trash2 size={14} />
      </button>
      <button
        type="button"
        aria-label={readLabel}
        title={readLabel}
        tabIndex={tabIndex}
        style={btn}
        onClick={stop(() => onAction('read'))}
      >
        {row.unreadCount > 0 ? <MailOpen size={14} /> : <Mail size={14} />}
      </button>
      {row.snoozedUntil ? (
        <button
          type="button"
          aria-label="Unsnooze — move back to Inbox"
          title="Unsnooze — move back to Inbox"
          tabIndex={tabIndex}
          style={btn}
          onClick={stop(() => onAction('unsnooze'))}
        >
          <BellOff size={14} />
        </button>
      ) : (
        <button
          type="button"
          aria-label="Snooze"
          title="Snooze (h)"
          tabIndex={tabIndex}
          style={btn}
          onClick={stop(() => onAction('snooze'))}
        >
          <Clock size={14} />
        </button>
      )}
    </span>
  );
}

export function rowSurface(focused: boolean, selected: boolean, hover: boolean): string {
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
  hovered,
  posInSet,
  setSize,
  onFocus,
  onToggleSelect,
  onOpen,
  onAction,
}: Props) {
  const avatars = useSettings((s) => s.settings.avatarsInList);
  const density = useSettings((s) => s.settings.density);
  const h = rowHeightForDensity(density);
  const unread = row.unreadCount > 0;
  // Chips carry real label names, resolved per account (P3.6): a raw
  // `Label_...`/`imap:...` id is not something a reader can act on, and the
  // same name in another account is a different label.
  const names = useLabels((s) => s.names);
  const chipIds = row.labelIds
    .filter((l) => !['INBOX', 'UNREAD', 'STARRED', 'SENT', 'DRAFT', 'SPAM', 'TRASH', 'IMPORTANT'].includes(l))
    .slice(0, 2);
  const overflow =
    row.labelIds.filter(
      (l) => !['INBOX', 'UNREAD', 'STARRED', 'SENT', 'DRAFT', 'SPAM', 'TRASH', 'IMPORTANT'].includes(l),
    ).length - chipIds.length;
  // A conversation the reader sent is headed by its recipients, not by the
  // account that sent it (P3.6). Falls back to the participant label when the
  // conversation holds nobody else.
  const headline =
    (row.labelIds.includes('SENT') && !row.labelIds.includes('INBOX') && accountLabel
      ? recipientsLabel(row.participants, [accountLabel])
      : null) ?? participantsLabel(row.participants, row.messageCount);
  const surface = rowSurface(focused, selected, hovered);
  const shell: React.CSSProperties = {
    height: h,
    maxHeight: h,
    overflow: 'hidden',
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    padding: density === 'compact' ? '0 12px 0 10px' : '4px 12px 4px 10px',
    background: surface,
    borderLeft: '2px solid transparent',
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

  // Row actions are drawn by the list as an overlay (P9.5); `onAction` stays in
  // the props so a row can still be used standalone in tests.
  void onAction;
  // Truncated rows carry the full text as a tooltip; ellipsis alone loses it.
  const headlineTitle = `${headline} — ${decodeRfc2047(row.subject)}`;

  if (density === 'compact') {
    return (
      <div
        role="option"
        id={rowDomId(row)}
        aria-selected={selected}
        aria-posinset={posInSet}
        aria-setsize={setSize}
        onClick={onClick}
        data-row-key={rowKey(row)}
        style={shell}
        className="sift-row"
        data-testid={`row-${row.id}`}
        title={showStripe && accountLabel ? accountLabel : undefined}
      >
        {showStripe && (
          <span
            aria-hidden
            data-testid="account-marker"
            style={{ ...ACCOUNT_DASH, background: accentHex(accountColor ?? 'blue'), opacity: 0.55 }}
          />
        )}
        {showStripe && accountLabel ? <span style={SR_ONLY}>Account: {accountLabel}</span> : null}
        {
          <span
            style={{
              width: 4,
              height: 4,
              borderRadius: '50%',
              background: unread ? 'var(--unread-dot)' : 'transparent',
              flexShrink: 0,
            }}
          />
        }
        <span
          title={headlineTitle}
          style={{
            fontWeight: unread ? 600 : 400,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            fontSize: 12.5,
          }}
        >
          {headline}
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
          <span style={{ color: 'var(--fg-row-meta)', fontWeight: 400 }}>· {decodeRfc2047(row.snippet)}</span>
        </span>
        {row.isStarred && (
          <Star
            data-testid="star-affordance"
            size={13}
            fill="var(--star)"
            color="var(--star)"
            style={{ flexShrink: 0, width: 13, height: 13 }}
          />
        )}
        {row.hasAttachments && (
          <Paperclip
            data-testid="attachment-affordance"
            size={13}
            color="var(--fg-row-meta)"
            style={{ flexShrink: 0, width: 13, height: 13 }}
          />
        )}
        <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-row-meta)', flexShrink: 0 }}>
          {formatRowDate(row.lastMessageAt)}
        </span>
      </div>
    );
  }

  return (
    <div
      role="option"
      id={rowDomId(row)}
      aria-selected={selected}
      aria-posinset={posInSet}
      aria-setsize={setSize}
      onClick={onClick}
      data-row-key={rowKey(row)}
      style={shell}
      className="sift-row"
      data-testid={`row-${row.id}`}
      title={showStripe && accountLabel ? accountLabel : undefined}
    >
      {showStripe && (
        <span
          aria-hidden
          data-testid="account-marker"
          style={{ ...ACCOUNT_DASH, background: accentHex(accountColor ?? 'blue'), opacity: 0.55 }}
        />
      )}
      {showStripe && accountLabel ? <span style={SR_ONLY}>Account: {accountLabel}</span> : null}
      {
        <span
          style={{
            width: 4,
            height: 4,
            borderRadius: '50%',
            background: unread ? 'var(--unread-dot)' : 'transparent',
            flexShrink: 0,
          }}
        />
      }
      {avatars && <Avatar email={row.participants[0]?.e ?? '?'} name={row.participants[0]?.n} size={24} />}
      <div style={{ flex: 1, minWidth: 0, overflow: 'hidden' }}>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 8 }}>
          <span
            title={headlineTitle}
            style={{
              fontWeight: unread ? 600 : 400,
              fontSize: 13,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              flex: 1,
            }}
          >
            {headline}
          </span>
          <span className="num" style={{ fontSize: 11.5, color: 'var(--fg-row-meta)', flexShrink: 0 }}>
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
              color: 'var(--fg-row-meta)',
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
              data-testid="star-affordance"
              size={14}
              fill="var(--star)"
              color="var(--star)"
              style={{ flexShrink: 0, width: 14, height: 14 }}
            />
          )}
          {row.hasAttachments && (
            <Paperclip
              data-testid="attachment-affordance"
              size={14}
              color="var(--fg-2)"
              style={{ flexShrink: 0, width: 14, height: 14 }}
            />
          )}
          {chipIds.map((c) => (
            <Chip key={c} label={names[labelKey(row.accountId, c)] ?? c} />
          ))}
          {overflow > 0 && <span style={{ fontSize: 11, color: 'var(--fg-row-meta)' }}>+{overflow}</span>}
        </div>
      </div>
    </div>
  );
});
