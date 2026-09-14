import { describe, it, expect } from 'vitest';
import { looksBinary, sourceLines } from './sourceView';

describe('source view helpers (P9.3)', () => {
  it('splits on newlines without normalising the content', () => {
    const raw = 'From: a@example.com\r\nSubject: hi\n\nbody line\n';
    const lines = sourceLines(raw);
    expect(lines).toEqual(['From: a@example.com\r', 'Subject: hi', '', 'body line', '']);
    // Nothing is trimmed or re-encoded: the lines are the source.
    expect(lines.join('\n')).toBe(raw);
  });

  it('flags a decoded source that is not clean text', () => {
    // Replacement characters mean the bytes were not valid UTF-8; a NUL means
    // the backend decoded something that was never text.
    expect(looksBinary('plain ascii')).toBe(false);
    expect(looksBinary('unicode: Grüße — ok')).toBe(false);
    expect(looksBinary('broken: \uFFFD\uFFFD')).toBe(true);
    expect(looksBinary('binary: \u0000\u0001')).toBe(true);
  });
});
