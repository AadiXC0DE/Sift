/**
 * Save a message's raw source as a `.eml` file (P9.3).
 *
 * The native chooser and the file write both happen in Rust; this module only
 * carries the outcome and the toast policy, so a cancelled chooser stays as
 * silent as `attachments_save_as` and the reader's menu stays wiring-only.
 */
import { toast } from 'sonner';
import { utilities } from '../mail-utilities/ipc';
import type { MenuItem } from '../../ui/Menu';

export interface SaveEmlResult {
  saved: boolean;
  path?: string | null;
}

/**
 * Returns `{saved: false}` when the chooser was dismissed: that is a normal
 * outcome, not a failure, exactly like `attachments_save_as`.
 */
export async function saveMessageAsEml({
  accountId,
  messageId,
}: {
  accountId: string;
  messageId: string;
}): Promise<SaveEmlResult> {
  const { path } = await utilities.message_raw_export({ accountId, messageId });
  if (!path) return { saved: false };
  return { saved: true, path };
}

/**
 * The reader's More-menu item. It owns the success/error copy so every caller
 * of "Save as .eml" reports the same thing.
 */
export function saveEmlMenuItem({
  accountId,
  messageId,
}: {
  accountId: string;
  messageId: string;
}): MenuItem {
  return {
    label: 'Save as .eml…',
    action: () => {
      void (async () => {
        try {
          const { saved, path } = await saveMessageAsEml({ accountId, messageId });
          if (!saved || !path) return;
          toast.success(`Saved ${path.split(/[\\/]/).pop() ?? path}`);
        } catch (e) {
          const text =
            e instanceof Error && e.message
              ? e.message
              : 'The raw message is not cached on this device. Connect and open the message once to download it, then try again.';
          // A cancelled save dialog is not an error.
          if (/cancel/i.test(text)) return;
          toast.error(text);
        }
      })();
    },
  };
}
