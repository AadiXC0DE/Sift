/**
 * Print dialog (P9.3).
 *
 * Printing happens in a hidden, same-origin iframe: a sandboxed frame cannot
 * open the system print panel. The document it loads is the sanitised message
 * HTML plus the print stylesheet, so printing never navigates the app and
 * never touches message state.
 */
import React, { useEffect, useState } from 'react';
import { Dialog } from '../../ui/Dialog';
import { Button } from '../../ui/Button';
import { Switch } from '../../ui/Switch';
import { buildPrintDocument } from './printDocument';
import type { PrintDocumentInput } from './printDocument';
import printCss from '../../styles/print.css?raw';

/** A frame that never reports `load` must not stall the print button (P9.3). */
const PRINT_LOAD_TIMEOUT_MS = 1500;
/** Web fonts refine metrics; they are not worth blocking the panel for. */
const FONT_WAIT_MS = 500;
/** Marks the frame so a dialog closed mid-print can still clean it up. */
const PRINT_FRAME_ATTR = 'data-sift-print';

/** Every print frame goes through here, so none can outlive the dialog. */
function removePrintFrames(): void {
  for (const frame of Array.from(document.querySelectorAll(`iframe[${PRINT_FRAME_ATTR}]`))) {
    frame.remove();
  }
}

/** A frame whose `contentWindow` (or `print`) the environment withheld cannot open the panel. */
function canPrint(win: Window | null): win is Window {
  return win !== null && typeof win.print === 'function';
}

/**
 * Build the document, load it into a same-origin frame and hand it to the
 * system print panel. `'blocked'` is returned when the environment has no
 * `contentWindow.print`, so the dialog can say so instead of failing silently.
 */
export async function printMessage(input: PrintDocumentInput): Promise<'printed' | 'blocked'> {
  const frame = document.createElement('iframe');
  frame.setAttribute(PRINT_FRAME_ATTR, '');
  frame.setAttribute('aria-hidden', 'true');
  frame.tabIndex = -1;
  // Off-screen rather than `display:none`: a frame with no layout box cannot
  // be printed by the engine.
  frame.style.position = 'fixed';
  frame.style.left = '-10000px';
  frame.style.top = '0';
  frame.style.width = '1px';
  frame.style.height = '1px';
  frame.style.border = '0';
  document.body.appendChild(frame);

  try {
    if (!canPrint(frame.contentWindow)) return 'blocked';
    const loaded = new Promise<void>((resolve) => {
      let timer = 0;
      const finish = () => {
        window.clearTimeout(timer);
        frame.removeEventListener('load', finish);
        resolve();
      };
      timer = window.setTimeout(finish, PRINT_LOAD_TIMEOUT_MS);
      frame.addEventListener('load', finish);
    });
    frame.srcdoc = buildPrintDocument(input);
    await loaded;

    const win = frame.contentWindow;
    if (!canPrint(win)) return 'blocked';
    const fonts = win.document.fonts?.ready;
    if (fonts) {
      await Promise.race([
        fonts.catch(() => undefined),
        new Promise<void>((resolve) => window.setTimeout(resolve, FONT_WAIT_MS)),
      ]);
    }
    // Focus first: some engines route the print panel to the focused frame.
    win.focus();
    win.print();
    return 'printed';
  } finally {
    frame.remove();
  }
}

export function PrintDialog({
  open,
  onClose,
  subject,
  from,
  to,
  cc,
  date,
  html,
  text,
}: {
  open: boolean;
  onClose: () => void;
  subject: string;
  from: string;
  to: string;
  cc?: string[];
  date: number;
  html?: string;
  text?: string;
}) {
  const [includeQuoted, setIncludeQuoted] = useState(false);
  const [printing, setPrinting] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      // Every opening starts from the defaults; a previous failure note must
      // not survive into a new dialog.
      setIncludeQuoted(false);
      setPrinting(false);
      setFailure(null);
      return;
    }
    // Closing without printing (or mid-print) must not leave a frame behind.
    removePrintFrames();
  }, [open]);

  const run = async () => {
    setPrinting(true);
    try {
      const outcome = await printMessage({
        subject,
        from,
        to,
        cc,
        date,
        html,
        text,
        includeQuoted,
        css: printCss,
        title: subject,
      });
      if (outcome === 'blocked') {
        setFailure('Printing is unavailable in this environment.');
        return;
      }
      onClose();
    } catch {
      setFailure('Sift could not build the print document for this message.');
    } finally {
      setPrinting(false);
    }
  };

  return (
    <Dialog open={open} onClose={onClose} title="Print">
      <p style={{ margin: '0 0 14px', fontSize: 13, lineHeight: 1.5, color: 'var(--fg-2)' }}>
        Prints a header with the subject, sender, recipients and date, then{' '}
        {html && html.trim() ? 'the message body as it was authored' : 'the message body as plain text'}.
        {includeQuoted ? ' Quoted replies are expanded.' : ' Quoted replies stay hidden.'}
      </p>
      <div style={{ marginBottom: 16 }}>
        <Switch checked={includeQuoted} onChange={setIncludeQuoted} label="Include collapsed quoted text" />
      </div>
      {failure && <p style={{ margin: '0 0 14px', fontSize: 12, color: 'var(--warning)' }}>{failure}</p>}
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
        <Button variant="secondary" onClick={onClose} disabled={printing}>
          Cancel
        </Button>
        <Button variant="primary" onClick={() => void run()} disabled={printing}>
          {printing ? 'Opening print…' : 'Print…'}
        </Button>
      </div>
    </Dialog>
  );
}
