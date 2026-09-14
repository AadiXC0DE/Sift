import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { PrintDialog } from './PrintDialog';

const dialog = (onClose: () => void = () => {}) => (
  <PrintDialog
    open
    onClose={onClose}
    subject="Quarterly report"
    from="Ada Lovelace <ada@example.com>"
    to="Grace Hopper <grace@example.com>"
    date={Date.UTC(2026, 8, 14, 15, 4)}
    html="<p>Hello</p>"
  />
);

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Print dialog (P9.3)', () => {
  it('describes what prints and starts with quoted text excluded', async () => {
    render(dialog());

    expect(document.body.textContent).toContain('Quoted replies stay hidden.');
    const toggle = screen.getByRole('switch');
    expect(toggle).toHaveAttribute('aria-checked', 'false');

    await userEvent.setup().click(toggle);
    expect(document.body.textContent).toContain('Quoted replies are expanded.');
  });

  it('reports that printing is unavailable instead of failing silently', async () => {
    const proto = HTMLIFrameElement.prototype as unknown as { contentWindow: Window | null };
    vi.spyOn(proto, 'contentWindow', 'get').mockReturnValue(null);
    const onClose = vi.fn();

    render(dialog(onClose));
    await userEvent.setup().click(screen.getByRole('button', { name: 'Print…' }));

    expect(await screen.findByText('Printing is unavailable in this environment.')).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    // The frame never outlives the attempt.
    await waitFor(() => expect(document.querySelectorAll('iframe[data-sift-print]')).toHaveLength(0));
  });
});
