import React, { useCallback, useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronUp, X } from 'lucide-react';
import { IconButton } from '../../ui/IconButton';
import { pushSurface } from '../../ui/overlayStack';
import { intendedFindTarget, listFinders, subscribeFrameFocus, type FindHit, type FrameFinder } from './find';

/**
 * Typing settles for this long before the frames are asked to search (P9.3).
 * Each request crosses into an iframe and re-wraps its text nodes, so a search
 * per keystroke would fight the user's typing and reflow the message twice. A
 * plain timer rather than `lib/debounce` because this one has to be cancelled
 * by the effect's cleanup — a search that lands after the bar closed would
 * repaint highlights over a message the reader has finished with.
 */
const FIND_DEBOUNCE_MS = 120;

/**
 * The frame find should search: the one the user was last reading, else the
 * first reader on screen. The registry is module state, not React state, so
 * this stays correct across a re-render that swapped the frame underneath.
 */
function currentFinder(): FrameFinder | null {
  const wanted = intendedFindTarget();
  const all = listFinders();
  return all.find((f) => f.messageId === wanted) ?? all[0] ?? null;
}

/**
 * Reader-local find bar (P9.3). It is deliberately one component rather than a
 * store: the query is only meaningful while the bar is open, and the frames own
 * the matches. Every request is debounced, and only the newest request may
 * write its answer into the counter — an earlier frame that answers late must
 * not overwrite the number for the text the user is now looking at.
 */
export function FindBar({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [query, setQuery] = useState('');
  const [hit, setHit] = useState<FindHit | null>(null);
  // Bumped when the frames focus/register, so the target is re-resolved for the
  // render that follows instead of being frozen at open time.
  const [, setFrameVersion] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const seq = useRef(0);

  const runSearch = useCallback(async (q: string, direction: 1 | -1, reset: boolean) => {
    const finder = currentFinder();
    const mine = ++seq.current;
    if (!finder) {
      setHit(null);
      return;
    }
    const result = await finder.find(q, direction, reset);
    if (seq.current === mine) setHit(result);
  }, []);

  // While the bar is open it owns Escape: the overlay stack closes it through
  // onClose, and `hasBlockingSurface()` keeps list/reader shortcuts from firing
  // while the user is typing a query.
  useEffect(() => {
    if (!open) return;
    return pushSurface({ kind: 'managed', close: onClose });
  }, [open, onClose]);

  useEffect(() => {
    if (!open) return;
    return subscribeFrameFocus(() => setFrameVersion((n) => n + 1));
  }, [open]);

  useEffect(() => {
    if (!open) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [open]);

  // Closing find must not leave highlights painted inside the mail: the frames
  // are not unmounted by this component, so they have to be told (P9.3).
  useEffect(() => {
    if (!open) return;
    return () => {
      for (const finder of listFinders()) finder.clear();
    };
  }, [open]);

  const target = open ? currentFinder() : null;

  useEffect(() => {
    if (!open) return;
    // A count that belongs to the previous query (or to the frame the user
    // just left) would be a lie while the new search is out.
    setHit(null);
    const timer = setTimeout(() => void runSearch(query, 1, true), FIND_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [open, query, target?.messageId, runSearch]);

  const step = (direction: 1 | -1) => void runSearch(query, direction, false);

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape') {
      // Escape closes find and nothing else: without stopping the event the
      // app-level handler would also back out of the conversation (P9.3).
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      step(e.shiftKey ? -1 : 1);
    }
  };

  if (!open) return null;

  const count = hit?.count ?? 0;
  const status = query
    ? !target
      ? 'No message open'
      : hit === null
        ? '…'
        : count === 0
          ? 'No matches'
          : `${hit.index} of ${count}`
    : '';

  return (
    <div
      style={{
        position: 'absolute',
        top: 10,
        right: 16,
        zIndex: 40,
        display: 'flex',
        alignItems: 'center',
        gap: 6,
        padding: '5px 6px 5px 8px',
        background: 'var(--bg-elevated)',
        boxShadow: 'var(--shadow-popover)',
        border: '1px solid var(--border)',
        borderRadius: 'var(--r-md)',
      }}
    >
      <input
        ref={inputRef}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder="Find in message"
        aria-label="Find in message"
        spellCheck={false}
        style={{
          width: 168,
          height: 26,
          padding: '0 8px',
          border: '1px solid var(--border-strong)',
          borderRadius: 'var(--r-sm)',
          background: 'var(--bg-pane)',
          color: 'var(--fg)',
          fontSize: 12,
          outline: 'none',
        }}
      />
      <span
        aria-live="polite"
        style={{ minWidth: 68, textAlign: 'right', fontSize: 12, color: 'var(--fg-3)' }}
      >
        {status}
      </span>
      <IconButton tip="Previous match" size="sm" disabled={count === 0} onClick={() => step(-1)}>
        <ChevronUp size={14} />
      </IconButton>
      <IconButton tip="Next match" size="sm" disabled={count === 0} onClick={() => step(1)}>
        <ChevronDown size={14} />
      </IconButton>
      <IconButton tip="Close find" size="sm" onClick={onClose}>
        <X size={14} />
      </IconButton>
    </div>
  );
}
