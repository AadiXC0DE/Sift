import React, { useMemo, useState } from 'react';
import { Dialog } from '../../ui/Dialog';
import { Kbd } from '../../ui/Kbd';
import { defaultBindings } from '../../keymap/defaults';

export function ShortcutHelp({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [q, setQ] = useState('');
  const rows = useMemo(
    () => defaultBindings.filter((b) => !q || b.action.toLowerCase().includes(q.toLowerCase())),
    [q],
  );
  return (
    <Dialog open={open} onClose={onClose} title="Keyboard shortcuts" width={560}>
      <input
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder="Filter…"
        style={{
          width: '100%',
          height: 30,
          marginBottom: 12,
          border: '1px solid var(--border)',
          borderRadius: 6,
          padding: '0 8px',
          background: 'var(--bg-raised)',
          color: 'var(--fg)',
        }}
      />
      <div style={{ maxHeight: 380, overflowY: 'auto', display: 'flex', flexDirection: 'column', gap: 2 }}>
        {rows.map((b, i) => (
          <div
            key={i}
            style={{ display: 'flex', justifyContent: 'space-between', fontSize: 13, padding: '4px 0' }}
          >
            <span>{b.action}</span>
            <Kbd>{b.key}</Kbd>
          </div>
        ))}
      </div>
    </Dialog>
  );
}
