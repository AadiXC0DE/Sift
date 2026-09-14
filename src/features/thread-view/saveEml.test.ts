import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { toast } from 'sonner';
import { utilities } from '../mail-utilities/ipc';
import { saveEmlMenuItem, saveMessageAsEml } from './saveEml';

vi.mock('sonner', () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock('../mail-utilities/ipc', () => ({
  utilities: { message_raw_export: vi.fn() },
}));

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Save as .eml (P9.3)', () => {
  it('treats a dismissed chooser as "not saved" rather than a failure', async () => {
    vi.mocked(utilities.message_raw_export).mockResolvedValue({ path: null });
    await expect(saveMessageAsEml({ accountId: 'a1', messageId: 'm1' })).resolves.toEqual({
      saved: false,
    });
    expect(utilities.message_raw_export).toHaveBeenCalledWith({ accountId: 'a1', messageId: 'm1' });
  });

  it('reports the written path on success', async () => {
    vi.mocked(utilities.message_raw_export).mockResolvedValue({ path: '/tmp/Quarterly report.eml' });
    await expect(saveMessageAsEml({ accountId: 'a1', messageId: 'm1' })).resolves.toEqual({
      saved: true,
      path: '/tmp/Quarterly report.eml',
    });
  });

  it('names the saved file, and stays silent when the chooser was cancelled', async () => {
    vi.mocked(utilities.message_raw_export).mockResolvedValue({ path: '/tmp/report.eml' });
    saveEmlMenuItem({ accountId: 'a1', messageId: 'm1' }).action();
    await vi.waitFor(() => expect(toast.success).toHaveBeenCalledWith('Saved report.eml'));

    vi.mocked(utilities.message_raw_export).mockRejectedValue(new Error('Save cancelled'));
    saveEmlMenuItem({ accountId: 'a1', messageId: 'm1' }).action();
    await vi.waitFor(() => expect(utilities.message_raw_export).toHaveBeenCalledTimes(2));
    expect(toast.error).not.toHaveBeenCalled();
  });

  it('explains that the raw message must be downloaded when the save fails', async () => {
    vi.mocked(utilities.message_raw_export).mockRejectedValue(new Error(''));
    saveEmlMenuItem({ accountId: 'a1', messageId: 'm1' }).action();
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        'The raw message is not cached on this device. Connect and open the message once to download it, then try again.',
      ),
    );
  });
});
