import { describe, expect, it } from 'vitest';
import { attachmentName } from './AttachmentStrip';
import { previewKind } from './AttachmentPreview';

describe('P2.6 preview kinds', () => {
  it('renders the types the system WebView handles', () => {
    expect(previewKind('application/pdf')).toBe('pdf');
    expect(previewKind('image/png')).toBe('image');
    expect(previewKind('IMAGE/JPEG; name=logo.jpg')).toBe('image');
  });

  it('never treats markup, archives or executables as renderable content', () => {
    const refused = [
      'text/html',
      'application/xhtml+xml',
      'application/javascript',
      'application/octet-stream',
      'application/zip',
      'text/plain',
      'message/rfc822',
      '',
    ];
    for (const mime of refused) expect(previewKind(mime), mime).toBeNull();
  });
});

describe('P2.6 attachment names', () => {
  it('keeps the filename the sender used, whatever the backend writes', () => {
    expect(
      attachmentName({ filename: 'Invoice final.pdf', mime: 'application/pdf', id: 'acc-a:m:att-1' }),
    ).toBe('Invoice final.pdf');
  });

  it('derives the same fallback name the backend writes for an unnamed part', () => {
    expect(
      attachmentName({
        filename: null,
        mime: 'application/octet-stream',
        id: 'acc-a:blue-00-m1:att-unnamed',
      }),
    ).toBe('attachment-accablue.bin');
  });
});
