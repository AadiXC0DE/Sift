import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { api } from '../../app/ipc/commands';
import type { StorageUsage } from '../../app/ipc/types';
import { StoragePanel } from './StoragePanel';

vi.mock('../../app/ipc/commands', () => ({
  api: { storage_usage: vi.fn(), storage_clear_attachment_cache: vi.fn() },
}));

const MIB = 1024 * 1024;

function usageWith(attachmentBytes: number): StorageUsage {
  return {
    metadata: { bytes: 2 * MIB, items: 1200 },
    bodies: { bytes: 8 * MIB, items: 340 },
    attachments: { bytes: attachmentBytes, items: attachmentBytes > 0 ? 4 : 0 },
    draftCache: { bytes: 1.5 * MIB, items: 2 },
    pinnedBytes: 0,
    attachmentCacheLimitBytes: 512 * MIB,
    totalBytes: 2 * MIB + 8 * MIB + attachmentBytes + 1.5 * MIB,
    computedAt: 1_700_000_000_000,
  };
}

const rowText = (key: string) => document.querySelector(`[data-storage-row="${key}"]`)?.textContent ?? '';

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Settings -> Storage (P10.4)', () => {
  it('shows measured usage for every cache class', async () => {
    vi.mocked(api.storage_usage).mockResolvedValue(usageWith(32 * MIB));
    render(<StoragePanel />);

    expect(await screen.findByText(/in local caches/)).toBeInTheDocument();
    expect(rowText('metadata')).toMatch(/2\.0 MiB/);
    expect(rowText('bodies')).toMatch(/8\.0 MiB/);
    expect(rowText('attachments')).toMatch(/32 MiB/);
    expect(rowText('draftCache')).toMatch(/1\.5 MiB/);
    expect(screen.getByText(/Attachment cache limit 512 MiB/)).toBeInTheDocument();
  });

  it('clears the downloaded attachment cache and shows the post-clear totals', async () => {
    vi.mocked(api.storage_usage).mockResolvedValue(usageWith(32 * MIB));
    vi.mocked(api.storage_clear_attachment_cache).mockResolvedValue(usageWith(0));
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const user = userEvent.setup();

    render(<StoragePanel />);
    expect(await screen.findByText(/in local caches/)).toBeInTheDocument();
    expect(rowText('attachments')).toMatch(/32 MiB/);

    await user.click(screen.getByRole('button', { name: 'Clear downloaded attachment cache' }));

    expect(rowText('attachments')).toMatch(/0 B/);
    expect(rowText('bodies')).toMatch(/8\.0 MiB/);
  });

  it('degrades gracefully when the cache backend is not registered yet', async () => {
    vi.mocked(api.storage_usage).mockRejectedValue(new Error('command storage_usage not found'));
    render(<StoragePanel />);

    expect(await screen.findByText('Storage details are not available in this build.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Clear downloaded attachment cache' })).toBeDisabled();
    expect(api.storage_clear_attachment_cache).not.toHaveBeenCalled();
  });
});
