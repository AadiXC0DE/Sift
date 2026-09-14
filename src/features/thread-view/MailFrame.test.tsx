import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render } from '@testing-library/react';
import { MailFrame } from './MailFrame';
import { api } from '../../app/ipc/commands';

vi.mock('../../app/ipc/commands', () => ({
  api: { app_open_url: vi.fn() },
}));

function srcdocOf(container: HTMLElement): string {
  return container.querySelector('iframe')!.getAttribute('srcdoc')!;
}

function frameOf(container: HTMLElement): HTMLIFrameElement {
  return container.querySelector('iframe')!;
}

/** Post a message as the frame would, so the parent's validation is exercised. */
function postFromFrame(
  container: HTMLElement,
  data: Record<string, unknown>,
  opts?: { token?: string | null; source?: unknown },
): void {
  const iframe = frameOf(container);
  const token = opts?.token === undefined ? tokenOf(container) : opts.token;
  const payload = { __sift: true, ...(token ? { token } : {}), ...data };
  const event = new MessageEvent('message', {
    data: payload,
    source: (opts?.source ?? iframe.contentWindow) as MessageEventSource,
  });
  window.dispatchEvent(event);
}

/** The token the shim was built with, read back out of the injected script. */
function tokenOf(container: HTMLElement): string {
  const doc = srcdocOf(container);
  const match = doc.match(/var TOKEN = "([0-9a-f]+)"/);
  if (!match) throw new Error('shim token missing');
  return match[1];
}

beforeEach(() => {
  delete document.documentElement.dataset.theme;
});

describe('P5-T04 CSP + P5-T05 link policy', () => {
  it('allows real-world mail image, media, CSS, and font sources', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const doc = srcdocOf(container);
    expect(doc).toMatch(/img-src data: blob: sift-att: http: https:/);
    expect(doc).toMatch(/media-src data: blob: sift-att: http: https:/);
    expect(doc).toMatch(/style-src 'unsafe-inline' http: https:/);
    expect(doc).toMatch(/font-src data: http: https:/);
  });
  it('honors an explicit remote-images block', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed={false} darkSafe />);
    const doc = srcdocOf(container);
    expect(doc).toMatch(/img-src data: blob: sift-att:;/);
    expect(doc).not.toMatch(/img-src[^;]*https:/);
  });
  it('keeps the iframe execution boundary locked down', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const iframe = frameOf(container);
    const doc = srcdocOf(container);
    expect(iframe.getAttribute('sandbox')).toBe('allow-scripts');
    expect(iframe.getAttribute('sandbox')).not.toMatch(/allow-same-origin/);
    expect(doc).toMatch(/default-src 'none'/);
    expect(doc).toMatch(/frame-src 'none'/);
    expect(doc).toMatch(/object-src 'none'/);
    expect(doc).toMatch(/form-action 'none'/);
  });
  it('javascript href rejected (handler never calls open)', () => {
    expect(vi.mocked(api.app_open_url)).not.toHaveBeenCalled();
  });
});

describe('P9.2 frame payload validation', () => {
  it('lets the app scheme serve cached CID content without embedding base64 bytes', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed={false} darkSafe />);
    const doc = srcdocOf(container);
    expect(doc).toMatch(/img-src data: blob: sift-att:/);
    expect(doc).toMatch(/media-src data: blob: sift-att:/);
  });

  it('sizes the frame from a token-carrying message and clamps the height', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const iframe = frameOf(container);

    act(() => {
      postFromFrame(container, { type: 'size', height: 240 });
    });
    expect(iframe.style.height).toBe('240px');

    // A nonsense height must not be able to grow the reader without bound.
    act(() => {
      postFromFrame(container, { type: 'size', height: 5_000_000 });
    });
    expect(iframe.style.height).toBe('20000px');
    act(() => {
      postFromFrame(container, { type: 'size', height: Number.NaN });
    });
    expect(iframe.style.height).toBe('20000px');
  });

  it('ignores a message without this frame token', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const iframe = frameOf(container);
    act(() => {
      postFromFrame(container, { type: 'size', height: 240 }, { token: 'not-the-token' });
    });
    expect(iframe.style.height).not.toBe('240px');
  });

  it('ignores a message from another window even with a matching token', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const iframe = frameOf(container);
    act(() => {
      postFromFrame(container, { type: 'size', height: 240 }, { source: window });
    });
    expect(iframe.style.height).not.toBe('240px');
  });

  it('forwards navigation keys only, and never a modifier chord', () => {
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    const seen: string[] = [];
    const listener = (e: KeyboardEvent) => seen.push(e.key);
    window.addEventListener('keydown', listener);
    act(() => {
      // j is navigation and is forwarded; x (select), c (compose) and a
      // modified chord are not, so mail HTML can never run an app command.
      postFromFrame(container, { type: 'key', key: 'j' });
      postFromFrame(container, { type: 'key', key: 'x' });
      postFromFrame(container, { type: 'key', key: 'c' });
      postFromFrame(container, { type: 'key', key: 'j', metaKey: true });
    });
    window.removeEventListener('keydown', listener);
    expect(seen).toEqual(['j']);
  });

  it('drops an over-long link instead of opening it', () => {
    vi.mocked(api.app_open_url).mockClear();
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    act(() => {
      postFromFrame(container, { type: 'link', href: `https://example.test/${'a'.repeat(4000)}` });
    });
    expect(vi.mocked(api.app_open_url)).not.toHaveBeenCalled();

    act(() => {
      postFromFrame(container, { type: 'link', href: 'https://example.test/ok' });
    });
    expect(vi.mocked(api.app_open_url)).toHaveBeenCalledWith('https://example.test/ok');
  });
});

describe('P9.2 message surface', () => {
  it('only follows the app theme for a dark-safe message', () => {
    document.documentElement.dataset.theme = 'dark';
    const safe = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    expect(srcdocOf(safe.container)).toContain('class="sift-mail sift-mail-dark"');

    // A newsletter with its own dark palette keeps a light surface: inherited
    // dark text on a dark background is unreadable.
    const unsafe = render(<MailFrame messageId="m2" html="<p>hi</p>" allowed darkSafe={false} />);
    expect(srcdocOf(unsafe.container)).toContain('class="sift-mail sift-mail-light"');
  });

  it('stays on the light surface when the app is light', () => {
    document.documentElement.dataset.theme = 'light';
    const { container } = render(<MailFrame messageId="m1" html="<p>hi</p>" allowed darkSafe />);
    expect(srcdocOf(container)).toContain('class="sift-mail sift-mail-light"');
  });
});

it('keeps authored white-table mail readable in a dark app', () => {
  document.documentElement.dataset.theme = 'dark';
  const { container } = render(
    <MailFrame
      messageId="newsletter"
      html='<table bgcolor="#ffffff"><tr><td>Invoice</td></tr></table>'
      allowed
      darkSafe
    />,
  );
  expect(srcdocOf(container)).toContain('class="sift-mail sift-mail-light"');
});
