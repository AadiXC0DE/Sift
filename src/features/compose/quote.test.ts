import { describe, it, expect } from 'vitest';
import { forwardHtml, plainTextHtml, quoteAttribution, quoteHtml, sanitizeQuoteHtml } from './quote';
import type { Address } from '../../app/ipc/types';

const BOSS: Address = { e: 'boss@x.com', n: 'Boss' };

describe('P5.4 quote sanitizing', () => {
  it('keeps ordinary formatting', () => {
    const html = '<p>Hello <strong>there</strong></p><ul><li>one</li></ul><blockquote>q</blockquote>';
    expect(sanitizeQuoteHtml(html)).toBe(html);
  });

  it('removes scripts, styles, frames and embedded content with their contents', () => {
    const html =
      '<p>keep</p><script>alert(1)</script><style>p{color:red}</style>' +
      '<iframe src="https://evil.test"></iframe><svg><path/></svg><form><input value="x"></form>';
    const out = sanitizeQuoteHtml(html);
    expect(out).toContain('keep');
    for (const needle of ['script', 'style', 'iframe', 'svg', 'form', 'input', 'evil.test']) {
      expect(out).not.toContain(needle);
    }
  });

  it('strips event handlers and unsafe attributes but keeps safe links', () => {
    const html =
      '<p onclick="steal()" style="color:red">text <a href="https://ok.test" onmouseover="x()">ok</a>' +
      '<a href="javascript:alert(1)">bad</a></p>';
    const out = sanitizeQuoteHtml(html);
    expect(out).not.toContain('onclick');
    expect(out).not.toContain('onmouseover');
    expect(out).not.toContain('style=');
    expect(out).toContain('href="https://ok.test"');
    expect(out).not.toContain('javascript:');
  });

  it('never keeps a remote image in a quote', () => {
    const out = sanitizeQuoteHtml('<p>hi</p><img src="https://tracker.test/1.png">');
    expect(out).toContain('hi');
    expect(out).not.toContain('tracker.test');
  });

  it('unwraps unknown markup instead of dropping its text', () => {
    expect(sanitizeQuoteHtml('<custom-tag>kept text</custom-tag>')).toBe('kept text');
  });
});

describe('P5.4 quote content', () => {
  it('quotes the full body, not the snippet, with sender and date attribution', () => {
    const long = `${'line of the real message. '.repeat(20)}END OF FULL BODY`;
    const quote = quoteHtml({ text: long, from: BOSS, date: 1_760_000_000_000 });
    expect(quote).toContain('END OF FULL BODY');
    expect(quote).toContain('Boss &lt;boss@x.com&gt;');
    expect(quote).toContain('wrote on');
    expect(quote).toContain('2025');
  });

  it('marks the quote collapsible while keeping its content in the body', () => {
    const quote = quoteHtml({ text: 'Quoted text', from: BOSS, date: 1 });
    expect(quote).toContain('<details data-sift-quote="1"');
    expect(quote).toContain('data-attribution="Boss &lt;boss@x.com&gt; wrote on');
    expect(quote).toContain('<blockquote>');
    expect(quote).toContain('Quoted text');
  });

  it('escapes plain text so markup in a message cannot become markup here', () => {
    const quote = quoteHtml({ text: '<script>alert(1)</script>', from: BOSS });
    expect(quote).not.toContain('<script>');
    expect(quote).toContain('&lt;script&gt;');
  });

  it('produces nothing when the source message has no readable content', () => {
    expect(quoteHtml({ from: BOSS })).toBe('');
    expect(quoteHtml({ html: '   ', from: BOSS })).toBe('');
  });

  it('attributes without a date when the message has none', () => {
    expect(quoteAttribution(BOSS)).toBe('Boss <boss@x.com> wrote:');
    expect(quoteAttribution({ e: 'x@y.z' })).toBe('x@y.z wrote:');
  });
});

describe('P5.4 forwarded content', () => {
  it('carries the whole body plus the envelope lines', () => {
    const full = `${'forwarded body line. '.repeat(30)}FORWARD END`;
    const out = forwardHtml(
      { from: BOSS, to: [{ e: 'me@x.com' }], subject: 'Quarterly report', date: 1_760_000_000_000 },
      { text: full },
    );
    expect(out).toContain('Forwarded message');
    expect(out).toContain('From: Boss &lt;boss@x.com&gt;');
    expect(out).toContain('Subject: Quarterly report');
    expect(out).toContain('To: me@x.com');
    expect(out).toContain('FORWARD END');
    expect(out).not.toContain('<details');
  });

  it('sanitizes an HTML forward the same way a reply is sanitized', () => {
    const out = forwardHtml(
      { from: BOSS, to: [], subject: 's' },
      { html: '<p>ok</p><script>bad()</script>' },
    );
    expect(out).toContain('ok');
    expect(out).not.toContain('script');
  });

  it('produces nothing when the forwarded message has no content', () => {
    expect(forwardHtml({ from: BOSS, to: [], subject: 's' }, {})).toBe('');
  });
});

describe('plainTextHtml', () => {
  it('splits blank-line separated paragraphs and escapes markup', () => {
    expect(plainTextHtml('one\n\n<b>two</b>')).toBe('<p>one</p><p>&lt;b&gt;two&lt;/b&gt;</p>');
  });
});
