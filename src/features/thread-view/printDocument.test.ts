import { describe, it, expect } from 'vitest';
import { format } from 'date-fns';
import { buildPrintDocument } from './printDocument';

const CSS = 'body { font-size: 11pt; }';

const build = (over: Partial<Parameters<typeof buildPrintDocument>[0]> = {}) =>
  buildPrintDocument({
    subject: 'Quarterly report',
    from: 'Ada Lovelace <ada@example.com>',
    to: 'Grace Hopper <grace@example.com>',
    date: Date.UTC(2026, 8, 14, 15, 4),
    html: '<p>Hello</p>',
    includeQuoted: false,
    css: CSS,
    ...over,
  });

const parse = (html: string) => new DOMParser().parseFromString(html, 'text/html');

const headerRows = (doc: Document) =>
  Array.from(doc.querySelectorAll('.sift-print-row')).map((row) => ({
    label: row.querySelector('.sift-print-label')?.textContent ?? '',
    value: row.querySelector('.sift-print-value')?.textContent ?? '',
  }));

describe('print document (P9.3)', () => {
  it('collapses quoted history only when quoting is excluded', () => {
    const quoted = '<p>New reply</p><div class="gmail_quote"><p>Old reply</p></div>';

    const collapsed = parse(build({ html: quoted, includeQuoted: false }));
    const hidden = collapsed.querySelector('details.sift-quote');
    expect(hidden).not.toBeNull();
    expect(hidden?.getAttribute('data-sift-quote')).toBe('collapsed');
    expect(hidden?.hasAttribute('open')).toBe(false);
    expect(hidden?.querySelector('summary')?.textContent).toBe('Show quoted text');
    // The quoted text is still in the document; the print rule hides it.
    expect(hidden?.textContent).toContain('Old reply');

    const expanded = parse(build({ html: quoted, includeQuoted: true }));
    const shown = expanded.querySelector('details.sift-quote');
    expect(shown?.hasAttribute('open')).toBe(true);
    expect(shown?.hasAttribute('data-sift-quote')).toBe(false);
  });

  it('never carries a script element into the printed document', () => {
    const doc = build({
      html: '<p>Before</p><script>alert("x")</script><SCRIPT SRC="x.js"></SCRIPT><p>After</p>',
    });
    expect(doc).not.toMatch(/<script/i);
    const parsed = parse(doc);
    expect(parsed.querySelectorAll('script')).toHaveLength(0);
    expect(parsed.body.textContent).toContain('Before');
    expect(parsed.body.textContent).toContain('After');
  });

  it('drops an unterminated script block instead of letting it swallow the message', () => {
    const doc = build({ html: '<p>Kept</p><script src="x.js">' });
    expect(doc).not.toMatch(/<script/i);
    expect(parse(doc).body.textContent).toContain('Kept');
  });

  it('prints From, Date and To, and Cc only when the message had one', () => {
    const rows = headerRows(parse(build()));
    expect(rows.map((r) => r.label)).toEqual(['From', 'Date', 'To']);
    expect(rows[0]?.value).toBe('Ada Lovelace <ada@example.com>');
    expect(rows[1]?.value).toBe(format(new Date(Date.UTC(2026, 8, 14, 15, 4)), 'PPPp'));
    expect(rows[1]?.value).not.toMatch(/Invalid/);
    expect(rows[2]?.value).toBe('Grace Hopper <grace@example.com>');
    expect(parse(build()).querySelector('.sift-print-subject')?.textContent).toBe('Quarterly report');

    const withCc = headerRows(
      parse(build({ cc: ['Alan Turing <alan@example.com>', 'Katherine Johnson <kj@example.com>'] })),
    );
    expect(withCc.map((r) => r.label)).toEqual(['From', 'Date', 'To', 'Cc']);
    expect(withCc[3]?.value).toBe('Alan Turing <alan@example.com>, Katherine Johnson <kj@example.com>');
  });

  it('says the date is unknown rather than printing an invalid one', () => {
    const rows = headerRows(parse(build({ date: Number.NaN })));
    expect(rows[1]?.value).toBe('Unknown date');
  });

  it('carries a long table body through unchanged', () => {
    const rows = Array.from({ length: 60 }, (_, i) => `<tr><td>Row ${i}</td><td>${i * 3}</td></tr>`);
    const doc = parse(build({ html: `<table><tbody>${rows.join('')}</tbody></table>`, includeQuoted: true }));
    const printed = Array.from(doc.querySelectorAll('.sift-print-body tr'));
    expect(printed).toHaveLength(60);
    expect(printed[59]?.textContent).toBe('Row 59177');
    expect(printed[0]?.querySelectorAll('td')).toHaveLength(2);
  });

  it('escapes a plain-text body and embeds the print stylesheet', () => {
    const doc = build({ html: undefined, text: '<b>not markup</b>\nsecond line', subject: 'Plain' });
    expect(doc).toContain(CSS);
    expect(doc).toMatch(/@page\s*\{\s*margin:\s*16mm;?\s*\}/);
    expect(doc).toContain('details.sift-quote[data-sift-quote="collapsed"] { display: none; }');
    const parsed = parse(doc);
    expect(parsed.querySelector('.sift-print-text')?.textContent).toBe('<b>not markup</b>\nsecond line');
    expect(parsed.querySelector('.sift-print-body b')).toBeNull();
  });
});
