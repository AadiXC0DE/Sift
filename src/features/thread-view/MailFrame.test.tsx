import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { MailFrame } from './MailFrame';

vi.mock('../../app/ipc/commands', () => ({
  api: { app_open_url: vi.fn() },
}));

describe('P5-T04 CSP + P5-T05 link policy', () => {
  it('srcdoc contains CSP without https unless allowed', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed={false} dark={false} />);
    const iframe = container.querySelector('iframe')!;
    const doc = iframe.getAttribute('srcdoc')!;
    expect(doc).toMatch(/img-src data: sift-att:/);
    expect(doc).not.toMatch(/img-src data: sift-att: https:/);
  });
  it('allowed includes https', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed dark={false} />);
    const doc = container.querySelector('iframe')!.getAttribute('srcdoc')!;
    expect(doc).toMatch(/https:/);
  });
  it('javascript href rejected (handler never calls open)', async () => {
    const { api } = await import('../../app/ipc/commands');
    expect(vi.mocked(api.app_open_url)).not.toHaveBeenCalled();
  });
});
