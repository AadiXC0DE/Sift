import React, { useMemo, useState } from 'react';
import { Command } from 'cmdk';
import { api } from '../../app/ipc/commands';
import type { Label } from '../../app/ipc/types';

export function LabelPicker({
  labels,
  selected,
  accountId,
  threadIds,
  mode,
  onDone,
}: {
  labels: Label[];
  selected: string[][];
  accountId: string;
  threadIds: string[];
  mode: 'label' | 'move';
  onDone: () => void;
}) {
  const [q, setQ] = useState('');
  const filtered = useMemo(
    () => labels.filter((l) => l.name.toLowerCase().includes(q.toLowerCase())),
    [labels, q],
  );
  const apply = async (labelId: string) => {
    if (mode === 'move') {
      await api.threads_action({ accountId, threadIds, action: { kind: 'moveTo', labelId } });
    } else {
      await api.threads_action({ accountId, threadIds, action: { kind: 'addLabel', labelId } });
    }
    onDone();
  };
  const mixed = (id: string) => {
    const has = selected.map((s) => s.includes(id));
    if (has.every(Boolean)) return true;
    if (has.every((x) => !x)) return false;
    return 'mixed' as const;
  };
  return (
    <Command label="Labels" style={{ minWidth: 260 }}>
      <Command.Input
        value={q}
        onValueChange={setQ}
        placeholder="Search or create…"
        style={{
          width: '100%',
          height: 30,
          border: '1px solid var(--border)',
          borderRadius: 6,
          padding: '0 8px',
        }}
      />
      <Command.List style={{ maxHeight: 260, overflowY: 'auto' }}>
        {filtered.map((l) => (
          <Command.Item
            key={l.id}
            value={l.id}
            onSelect={() => void apply(l.id)}
            style={{ display: 'flex', gap: 8, padding: '6px 8px', fontSize: 13 }}
          >
            <input
              type="checkbox"
              checked={mixed(l.id) === true}
              ref={(el) => {
                if (el) el.indeterminate = mixed(l.id) === 'mixed';
              }}
              readOnly
            />
            {l.name}
          </Command.Item>
        ))}
        {filtered.length === 0 && q && (
          <button
            onClick={() => void api.labels_create(accountId, q).then((l) => void apply(l.id))}
            style={{ padding: 8, fontSize: 13 }}
          >
            Create &apos;{q}&apos;
          </button>
        )}
      </Command.List>
    </Command>
  );
}
