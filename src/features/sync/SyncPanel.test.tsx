import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SyncPanel, SyncInlineBar } from './SyncPanel';

describe('first-run sync feedback', () => {
  it('shows a determinate count while downloading', () => {
    render(
      <SyncPanel progress={{ active: true, phase: 'metadata', done: 1200, total: 5000, failed: null }} />,
    );
    expect(screen.getByText('Getting your mail')).toBeInTheDocument();
    expect(screen.getByText('1,200 of 5,000 conversations')).toBeInTheDocument();
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '1200');
  });

  it('shows an indeterminate listing state before totals are known', () => {
    render(<SyncPanel progress={{ active: true, phase: 'listing', done: 0, total: 0, failed: null }} />);
    expect(screen.getByText('Listing your conversations…')).toBeInTheDocument();
  });

  it('shows a retry affordance when sync failed', () => {
    render(
      <SyncPanel
        progress={{ active: false, phase: '', done: 0, total: 0, failed: 'offline' }}
        onRetry={() => {}}
      />,
    );
    expect(screen.getByText('Sync paused')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument();
  });

  it('hides the inline bar when idle and shows counts when active', () => {
    const { container, rerender } = render(
      <SyncInlineBar progress={{ active: false, phase: '', done: 0, total: 0, failed: null }} />,
    );
    expect(container.firstChild).toBeNull();
    rerender(
      <SyncInlineBar progress={{ active: true, phase: 'metadata', done: 40, total: 100, failed: null }} />,
    );
    expect(screen.getByText('40 / 100')).toBeInTheDocument();
  });
});
