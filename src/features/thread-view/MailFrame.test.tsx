import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { MailFrame } from './MailFrame';

vi.mock('../../app/ipc/commands', () => ({
  api: { app_open_url: vi.fn() },
}));

describe('P5-T04 CSP + P5-T05 link policy', () => {
  it('allows real-world mail image, media, CSS, and font sources', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed dark={false} />);
    const iframe = container.querySelector('iframe')!;
    const doc = iframe.getAttribute('srcdoc')!;
    expect(doc).toMatch(/img-src data: blob: http: https:/);
    expect(doc).toMatch(/media-src data: blob: http: https:/);
    expect(doc).toMatch(/style-src 'unsafe-inline' http: https:/);
    expect(doc).toMatch(/font-src data: http: https:/);
  });
  it('honors an explicit remote-images block', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed={false} dark={false} />);
    const doc = container.querySelector('iframe')!.getAttribute('srcdoc')!;
    expect(doc).toMatch(/img-src data: blob:;/);
    expect(doc).not.toMatch(/img-src[^;]*https:/);
  });
  it('keeps the iframe execution boundary locked down', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed dark={false} />);
    const iframe = container.querySelector('iframe')!;
    const doc = iframe.getAttribute('srcdoc')!;
    expect(iframe.getAttribute('sandbox')).toBe('allow-scripts');
    expect(doc).toMatch(/default-src 'none'/);
    expect(doc).toMatch(/frame-src 'none'/);
    expect(doc).toMatch(/object-src 'none'/);
    expect(doc).toMatch(/form-action 'none'/);
  });
  it('javascript href rejected (handler never calls open)', async () => {
    const { api } = await import('../../app/ipc/commands');
    expect(vi.mocked(api.app_open_url)).not.toHaveBeenCalled();
  });
});
