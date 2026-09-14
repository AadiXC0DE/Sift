import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { api } from '../../app/ipc/commands';
import { ViewSourceDialog } from './ViewSourceDialog';

vi.mock('../../app/ipc/commands', () => ({
  api: { message_raw_source: vi.fn() },
}));

const open = () => (
  <ViewSourceDialog open onClose={() => {}} accountId="a1" messageId="m1" subject="Quarterly report" />
);

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('View Source (P9.3)', () => {
  it('explains what is missing when the raw message is not cached, then retries', async () => {
    vi.mocked(api.message_raw_source).mockRejectedValueOnce(new Error('message not in store'));
    vi.mocked(api.message_raw_source).mockResolvedValueOnce('Subject: hi\n\nbody');
    const user = userEvent.setup();

    render(open());
    expect(await screen.findByText(/not cached on this device/)).toBeInTheDocument();
    expect(screen.getByText(/open this message once to download it/)).toBeInTheDocument();
    expect(api.message_raw_source).toHaveBeenCalledWith('a1', 'm1');

    await user.click(screen.getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('3 lines')).toBeInTheDocument();
    expect(api.message_raw_source).toHaveBeenCalledTimes(2);
  });

  it('warns that a source with undecodable bytes is not the original', async () => {
    vi.mocked(api.message_raw_source).mockResolvedValue('Subject: \uFFFD\n\nbody');
    render(open());

    expect(await screen.findByText(/U\+FFFD replacement characters/)).toBeInTheDocument();
    expect(screen.getByText('3 lines')).toBeInTheDocument();
  });

  it('says the source is decoded UTF-8 when nothing was lost', async () => {
    vi.mocked(api.message_raw_source).mockResolvedValue('Subject: hi\n\nbody');
    render(open());

    expect(await screen.findByText(/decoded UTF-8 text/)).toBeInTheDocument();
    expect(screen.queryByText(/U\+FFFD/)).toBeNull();
  });
});
