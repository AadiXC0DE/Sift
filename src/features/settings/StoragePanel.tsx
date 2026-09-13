import React from 'react';
import { toast } from 'sonner';
import { api } from '../../app/ipc/commands';
import type { StorageUsage } from '../../app/ipc/types';
import { Button } from '../../ui/Button';
import { formatBytes } from '../../lib/bytes';

type Bucket = 'metadata' | 'bodies' | 'attachments' | 'draftCache';

const CATEGORIES: { key: Bucket; label: string; hint: string }[] = [
  { key: 'metadata', label: 'Mail metadata', hint: 'Accounts, threads, labels, search index' },
  { key: 'bodies', label: 'Message bodies', hint: 'Downloaded bodies kept for offline reading' },
  { key: 'attachments', label: 'Downloaded attachments', hint: 'Cleared by the action below' },
  { key: 'draftCache', label: 'Draft staging', hint: 'Files attached to unsent drafts' },
];

export function StoragePanel() {
  const [usage, setUsage] = React.useState<StorageUsage | null>(null);
  const [unavailable, setUnavailable] = React.useState(false);
  const [clearing, setClearing] = React.useState(false);

  React.useEffect(() => {
    let alive = true;
    api
      .storage_usage()
      .then((u) => {
        if (alive) setUsage(u);
      })
      .catch(() => {
        // The cache-metering backend is not registered yet; show the panel
        // without counts instead of failing the whole Settings dialog.
        if (alive) setUnavailable(true);
      });
    return () => {
      alive = false;
    };
  }, []);

  const clearAttachmentCache = async () => {
    const ok = window.confirm(
      'Clear downloaded attachments? Cached files download again next time you open them.',
    );
    if (!ok) return;
    setClearing(true);
    try {
      // The command returns the post-clear totals, so the panel never guesses.
      setUsage(await api.storage_clear_attachment_cache());
      toast.success('Attachment cache cleared');
    } catch {
      toast.error('Could not clear the attachment cache');
    } finally {
      setClearing(false);
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <div style={{ fontSize: 13, color: 'var(--fg-2)' }}>
        {usage ? `${formatBytes(usage.totalBytes)} in local caches.` : 'Measuring local caches…'}
      </div>
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          border: '1px solid var(--border)',
          borderRadius: 'var(--r-md)',
        }}
      >
        {CATEGORIES.map((c, i) => (
          <div
            key={c.key}
            data-storage-row={c.key}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: 12,
              padding: '8px 12px',
              borderTop: i > 0 ? '1px solid var(--border)' : 'none',
            }}
          >
            <span style={{ display: 'flex', flexDirection: 'column' }}>
              <span style={{ fontSize: 13 }}>{c.label}</span>
              <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>{c.hint}</span>
            </span>
            <span
              style={{
                fontSize: 13,
                fontVariantNumeric: 'tabular-nums',
                color: usage ? 'var(--fg)' : 'var(--fg-3)',
              }}
            >
              {usage ? formatBytes(usage[c.key].bytes) : '—'}
            </span>
          </div>
        ))}
      </div>
      {usage && usage.attachmentCacheLimitBytes > 0 && (
        <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>
          Attachment cache limit {formatBytes(usage.attachmentCacheLimitBytes)} — changed under Advanced.
        </div>
      )}
      {unavailable && (
        <div role="status" style={{ fontSize: 12, color: 'var(--fg-3)' }}>
          Storage details are not available in this build.
        </div>
      )}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6, alignItems: 'flex-start' }}>
        <Button
          variant="danger"
          onClick={() => void clearAttachmentCache()}
          disabled={unavailable || clearing}
        >
          {clearing ? 'Clearing…' : 'Clear downloaded attachment cache'}
        </Button>
        <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>
          Saved files, drafts and pending sends are never deleted.
        </span>
      </div>
    </div>
  );
}
