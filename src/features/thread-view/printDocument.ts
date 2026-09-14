/**
 * Standalone print document builder (P9.3).
 *
 * The print frame is a separate document: it inherits no app styles, no tokens
 * and no chrome, so everything a printed message needs has to be inside the
 * string this module returns. It is deliberately DOM-based (not string
 * surgery) for the quote collapsing, because the reader collapses quotes the
 * same way and a regex cannot match nested `blockquote` elements.
 */
import { format } from 'date-fns';
import { escapeHtml } from '../compose/plainText';

/**
 * The quote containers the reader collapses. Kept in lockstep with the frame
 * shim (`shim.ts`) so the printed page hides exactly what the reader hides.
 */
const QUOTE_SELECTORS = [
  '.gmail_quote',
  '.yahoo_quoted',
  '[id^="divRplyFwdMsg"]',
  'blockquote[type="cite"]',
  'blockquote.cite',
  '#divRplyFwdMsg',
];

/**
 * Page-level rules. They live here, not in `print.css`, because they encode
 * the "include quoted text" decision and the page geometry of the print job
 * rather than the look of a message.
 */
const PRINT_RULES = `
html, body { margin: 0; padding: 0; background: #fff; color: #000; }
@page { margin: 16mm; }
table, thead, tbody, tfoot, tr, td, th, img { page-break-inside: avoid; break-inside: avoid; }
/* A printed <details> whose content is closed still prints the summary, so a
   collapsed quote is hidden outright instead of relying on the element. */
details.sift-quote[data-sift-quote="collapsed"] { display: none; }
`;

export interface PrintDocumentInput {
  subject: string;
  /** Already formatted for display, e.g. `Ada Lovelace <ada@example.com>`. */
  from: string;
  to: string;
  /** Printed only when the message actually had Cc recipients. */
  cc?: string[];
  /** `internalDate` in ms. */
  date: number;
  html?: string;
  text?: string;
  /** When false, quoted history is collapsed and hidden from the printed page. */
  includeQuoted: boolean;
  /** `print.css?raw`; embedded verbatim. */
  css: string;
  title?: string;
}

/**
 * Remove script blocks before the HTML is embedded.
 *
 * The backend already sanitises message HTML, but the print document is built
 * separately from the reader's frame and gets no shim, so the guarantee is
 * restated here: complete blocks go, and so does an unterminated `<script`,
 * which would otherwise swallow the rest of the message when parsed.
 */
function stripScripts(html: string): string {
  return html
    .replace(/<script\b[^>]*>[\s\S]*?<\/script\s*>/gi, '')
    .replace(/<script\b[\s\S]*$/i, '')
    .replace(/<\/script\s*>/gi, '');
}

/**
 * Wrap every quoted-history container in `details.sift-quote`, the rule the
 * reader uses. `includeQuoted` opens the details; a collapsed one also carries
 * `data-sift-quote="collapsed"` so the print stylesheet can hide it.
 */
function collapseQuotes(root: HTMLElement, includeQuoted: boolean): void {
  const doc = root.ownerDocument;
  for (const node of Array.from(root.querySelectorAll(QUOTE_SELECTORS.join(', ')))) {
    // A container already inside a quote wrapper moves with its parent.
    if (node.closest('details.sift-quote')) continue;
    const wrap = doc.createElement('details');
    wrap.className = 'sift-quote';
    if (includeQuoted) wrap.setAttribute('open', '');
    else wrap.setAttribute('data-sift-quote', 'collapsed');
    const summary = doc.createElement('summary');
    summary.textContent = 'Show quoted text';
    node.parentNode?.insertBefore(wrap, node);
    wrap.append(summary, node);
  }
}

/** The message body: sanitised HTML when the message has a formatted part, otherwise escaped plain text. */
function buildBody(html: string | undefined, text: string | undefined, includeQuoted: boolean): string {
  if (html && html.trim()) {
    const parsed = new DOMParser().parseFromString(stripScripts(html), 'text/html');
    for (const script of Array.from(parsed.querySelectorAll('script'))) script.remove();
    collapseQuotes(parsed.body, includeQuoted);
    return `<div class="sift-print-body">${parsed.body.innerHTML}</div>`;
  }
  return `<div class="sift-print-body"><pre class="sift-print-text">${escapeHtml(text ?? '')}</pre></div>`;
}

export function buildPrintDocument(input: PrintDocumentInput): string {
  const { subject, from, to, date, html, text, includeQuoted, css, title } = input;
  const cc = input.cc ?? [];
  const rows: [string, string][] = [
    ['From', from],
    ['Date', Number.isFinite(date) ? format(new Date(date), 'PPPp') : 'Unknown date'],
    ['To', to],
  ];
  if (cc.length) rows.push(['Cc', cc.join(', ')]);
  const meta = rows
    .map(
      ([label, value]) =>
        `    <div class="sift-print-row"><span class="sift-print-label">${escapeHtml(label)}</span><span class="sift-print-value">${escapeHtml(value)}</span></div>`,
    )
    .join('\n');

  return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(title ?? subject)}</title>
<style>${css}${PRINT_RULES}</style>
</head>
<body>
  <header class="sift-print-head">
    <h1 class="sift-print-subject">${escapeHtml(subject)}</h1>
${meta}
  </header>
${buildBody(html, text, includeQuoted)}
</body>
</html>
`;
}
