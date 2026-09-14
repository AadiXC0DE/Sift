import type { Address } from '../../app/ipc/types';
import { escapeHtml } from './plainText';

/**
 * Quote construction for replies and forwards (P5.4).
 *
 * The quoted body is the real readable message, never the list snippet. HTML is
 * sanitized against a fixed allowlist before it reaches TipTap: the quote is
 * rendered inside the composer and sent as-is, so anything that could run or
 * phone home must be gone before it is inserted.
 */

/** Elements kept in a quote. Everything else is unwrapped to its text. */
const KEEP = new Set([
  'p',
  'br',
  'div',
  'span',
  'hr',
  'a',
  'ul',
  'ol',
  'li',
  'blockquote',
  'pre',
  'code',
  'strong',
  'b',
  'em',
  'i',
  'u',
  's',
  'sub',
  'sup',
  'small',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'table',
  'thead',
  'tbody',
  'tfoot',
  'tr',
  'td',
  'th',
  'caption',
]);

/** Elements dropped with their content: nothing inside them is ever shown. */
const DROP = new Set([
  'script',
  'style',
  'head',
  'title',
  'meta',
  'link',
  'base',
  'template',
  'noscript',
  'iframe',
  'frame',
  'frameset',
  'object',
  'embed',
  'applet',
  'form',
  'input',
  'button',
  'select',
  'option',
  'textarea',
  'svg',
  'math',
  'canvas',
  'video',
  'audio',
  'source',
  'track',
  'img',
  'picture',
  'map',
  'area',
]);

/** Attribute allowlist per element; every other attribute is stripped. */
const ATTRS: Record<string, string[]> = {
  a: ['href', 'title'],
  table: ['colspan', 'rowspan'],
  td: ['colspan', 'rowspan'],
  th: ['colspan', 'rowspan'],
};

function isSafeHref(href: string): boolean {
  return /^(https?:|mailto:)/i.test(href.trim());
}

/**
 * Remove anything executable, remote or unreliable from untrusted message HTML
 * and return the cleaned fragment.
 */
export function sanitizeQuoteHtml(html: string): string {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  const elements = [...doc.body.querySelectorAll('*')];
  for (const el of elements) {
    const tag = el.tagName.toLowerCase();
    if (DROP.has(tag)) {
      el.remove();
      continue;
    }
    if (!KEEP.has(tag)) {
      const parent = el.parentNode;
      if (parent) {
        while (el.firstChild) parent.insertBefore(el.firstChild, el);
        el.remove();
      }
      continue;
    }
    const allowed = ATTRS[tag] ?? [];
    for (const attr of [...el.attributes]) {
      const name = attr.name.toLowerCase();
      if (!allowed.includes(name)) {
        el.removeAttribute(attr.name);
        continue;
      }
      if (name === 'href' && !isSafeHref(attr.value)) el.removeAttribute(attr.name);
    }
  }
  return doc.body.innerHTML;
}

/** Plain text into paragraphs, escaped so it survives the HTML pipeline. */
export function plainTextHtml(text: string): string {
  const paragraphs = text
    .replace(/\r\n?/g, '\n')
    .split(/\n{2,}/)
    .map((block) => block.trim())
    .filter(Boolean);
  if (!paragraphs.length) return '';
  return paragraphs.map((block) => `<p>${escapeHtml(block).replace(/\n/g, '<br>')}</p>`).join('');
}

export interface QuoteSource {
  html?: string;
  text?: string;
  from: Address;
  /** Original message date, when the thread carries one. */
  date?: number;
}

/** "Jane Doe <jane@example.test> wrote on 1 Jan 2026, 09:00:" */
export function quoteAttribution(from: Address, date?: number): string {
  const who = from.n?.trim() ? `${from.n} <${from.e}>` : from.e;
  if (!date) return `${who} wrote:`;
  return `${who} wrote on ${new Date(date).toLocaleString()}:`;
}

/**
 * The quoted block: a collapsed `<details>` whose content is still part of the
 * body, so a collapsed quote is transmitted in full (P5.4). The markup matches
 * the `siftQuote` editor node, which keeps the body in the document while the
 * summary is collapsed.
 */
export function quoteHtml(source: QuoteSource): string {
  const body = source.html?.trim() ? sanitizeQuoteHtml(source.html) : plainTextHtml(source.text ?? '');
  if (!body) return '';
  const attribution = escapeHtml(quoteAttribution(source.from, source.date)).replace(/"/g, '&quot;');
  return `<details data-sift-quote="1" data-attribution="${attribution}"><blockquote>${body}</blockquote></details>`;
}

export interface ForwardSource {
  from: Address;
  to: Address[];
  subject: string;
  date?: number;
}

/**
 * A forward carries the whole original message: the envelope lines the reader
 * expects, then the full sanitized body. Returned expanded, so what the user
 * sees in the composer is what is transmitted.
 */
export function forwardHtml(parent: ForwardSource, body: { html?: string; text?: string }): string {
  const content = body.html?.trim() ? sanitizeQuoteHtml(body.html) : plainTextHtml(body.text ?? '');
  if (!content) return '';
  const lines = [
    `From: ${parent.from.n ? `${parent.from.n} <${parent.from.e}>` : parent.from.e}`,
    parent.date ? `Date: ${new Date(parent.date).toLocaleString()}` : '',
    `Subject: ${parent.subject}`,
    parent.to.length ? `To: ${parent.to.map((a) => (a.n ? `${a.n} <${a.e}>` : a.e)).join(', ')}` : '',
  ].filter(Boolean);
  const header = lines.map((l) => escapeHtml(l)).join('<br>');
  return `<p>---------- Forwarded message ----------</p><p>${header}</p><blockquote>${content}</blockquote>`;
}
