/**
 * View Source dialog (P9.3).
 *
 * Shows the raw RFC 822 source the backend has cached for one message. The
 * source is decoded text, never the exact bytes, and the dialog says so: it
 * shows a warning when decoding already lost information (U+FFFD / NUL) and a
 * quieter note otherwise. There is no "copy as bytes" control, because no such
 * view exists to copy.
 *
 * A 5 MB source must not become 100k DOM nodes, so the lines are virtualized.
 */
import React, { useEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { api } from '../../app/ipc/commands';
import { Dialog } from '../../ui/Dialog';
import { Button } from '../../ui/Button';
import { Spinner } from '../../ui/Spinner';
import { looksBinary, sourceLines } from './sourceView';

/** Row height in px; the virtualizer needs a fixed estimate for plain lines. */
const LINE_HEIGHT = 18;

const NOT_CACHED_COPY =
  'The raw message is not cached on this device. Connect to the internet and open this message once to download it, then try again.';

const MONO = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';

/**
 * One raw-source read per message, shared across React StrictMode's double
 * mount (the pattern `StepConnecting` uses for sign-in). A source can run to
 * megabytes, so two IPC reads of the same message would decode it twice for
 * nothing. `fresh` (the Retry button) deliberately starts a new read.
 */
const inflight = new Map<string, Promise<string>>();

function readRawSource(accountId: string, messageId: string, fresh: boolean): Promise<string> {
  const key = `${accountId}\u0000${messageId}`;
  if (fresh) inflight.delete(key);
  const pending = inflight.get(key);
  if (pending) return pending;
  const read = api.message_raw_source(accountId, messageId);
  const entry = read.finally(() => {
    // Only drop our own entry: a retry may already have replaced it.
    if (inflight.get(key) === entry) inflight.delete(key);
  });
  inflight.set(key, entry);
  return entry;
}

/** What a failed backend said, when it said anything at all. */
function errorDetail(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  return typeof e === 'string' ? e : '';
}

export function ViewSourceDialog({
  open,
  onClose,
  accountId,
  messageId,
  subject,
}: {
  open: boolean;
  onClose: () => void;
  accountId: string;
  messageId: string;
  subject: string;
}) {
  const [raw, setRaw] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    // Clear first: a failed read must not leave the previous message's source
    // on screen behind its own error.
    setRaw(null);
    setError(null);
    readRawSource(accountId, messageId, attempt > 0).then(
      (text) => {
        if (!cancelled) setRaw(text ?? '');
      },
      (e: unknown) => {
        if (!cancelled) setError(errorDetail(e));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [open, accountId, messageId, attempt]);

  const lines = useMemo(() => (raw === null ? null : sourceLines(raw)), [raw]);
  const virtual = useVirtualizer({
    count: lines?.length ?? 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => LINE_HEIGHT,
    overscan: 24,
  });
  const binary = raw !== null && looksBinary(raw);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={subject ? `Message source — ${subject}` : 'Message source'}
      width={860}
    >
      {error !== null ? (
        <div style={{ padding: '4px 0 0' }}>
          <p style={{ margin: '0 0 6px', fontSize: 13, color: 'var(--fg)' }}>
            {/not found|unknown command/i.test(error)
              ? 'Raw source is not available in this build.'
              : NOT_CACHED_COPY}
          </p>
          {error && <p style={{ margin: '0 0 12px', fontSize: 12, color: 'var(--fg-3)' }}>{error}</p>}
          <Button variant="secondary" size="sm" onClick={() => setAttempt((n) => n + 1)}>
            Retry
          </Button>
        </div>
      ) : lines === null ? (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            gap: 8,
            height: 180,
            fontSize: 13,
            color: 'var(--fg-3)',
          }}
        >
          <Spinner size={16} />
          Loading message source…
        </div>
      ) : (
        <>
          <div className="num" style={{ marginBottom: 6, fontSize: 12, color: 'var(--fg-2)' }}>
            {lines.length.toLocaleString()} lines
          </div>
          <p
            style={{
              margin: '0 0 8px',
              fontSize: 11.5,
              lineHeight: 1.45,
              color: binary ? 'var(--warning)' : 'var(--fg-3)',
            }}
          >
            {binary
              ? 'This is decoded text, not the original bytes: the message contained byte sequences that are not valid UTF-8, shown here as U+FFFD replacement characters.'
              : 'Shown as decoded UTF-8 text. The message’s own encoding was converted for display, so this is not a byte-exact copy of its source.'}
          </p>
          <div
            ref={scrollRef}
            tabIndex={0}
            role="region"
            aria-label="Raw message source"
            style={{
              height: '60vh',
              maxHeight: 560,
              overflow: 'auto',
              background: 'var(--bg-raised)',
              border: '1px solid var(--border)',
              borderRadius: 'var(--r-md)',
              padding: '8px 10px',
              userSelect: 'text',
              cursor: 'text',
            }}
          >
            <div style={{ position: 'relative', height: virtual.getTotalSize() }}>
              {virtual.getVirtualItems().map((item) => (
                <div
                  key={item.key}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    height: item.size,
                    transform: `translateY(${item.start}px)`,
                    fontFamily: MONO,
                    fontSize: 12,
                    lineHeight: `${LINE_HEIGHT}px`,
                    whiteSpace: 'pre',
                  }}
                >
                  {lines[item.index] ?? ''}
                </div>
              ))}
            </div>
          </div>
        </>
      )}
    </Dialog>
  );
}
