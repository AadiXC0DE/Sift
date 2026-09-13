import { describe, it, expect } from 'vitest';
import { escapeHtml, htmlToPlainText } from './plainText';
import { parseAddressList } from './recipients';

describe('P5.5 quote-safe paste', () => {
  it('turns a pasted address list with a quoted comma name into two chips', () => {
    const pasted = 'Ada Lovelace <ada@x.com>, "Doe, John" <john@x.com>';
    expect(parseAddressList(pasted)).toEqual([
      { n: 'Ada Lovelace', e: 'ada@x.com' },
      { n: 'Doe, John', e: 'john@x.com' },
    ]);
  });
});

describe('P5.5 plain-text paste', () => {
  it('keeps only the text of pasted markup', () => {
    expect(htmlToPlainText('<p>Hello <b>bold</b></p><script>alert(1)</script>')).toContain('Hello bold');
    expect(htmlToPlainText('<p>Hello <b>bold</b></p>')).not.toContain('<b>');
  });

  it('escapes markup so it cannot re-enter as formatting', () => {
    expect(escapeHtml('a <b> & c')).toBe('a &lt;b&gt; &amp; c');
  });
});
