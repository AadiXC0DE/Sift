import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Command as Cmdk } from 'cmdk';
import { filterCommands } from './registry';
import { Kbd } from '../../ui/Kbd';

export function Palette({
  onClose,
  onCompose,
  onSettings,
}: {
  onClose: () => void;
  onCompose: () => void;
  onSettings: () => void;
}) {
  const [q, setQ] = useState('');
  const results = useMemo(() => filterCommands(q).slice(0, 30), [q]);
  const [hi, setHi] = useState(0);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setHi(0);
  }, [q]);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setHi((v) => Math.min(results.length - 1, v + 1));
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setHi((v) => Math.max(0, v - 1));
      }
      if (e.key === 'Enter') {
        const c = results[hi];
        if (c) {
          if (c.id === 'compose') onCompose();
          else if (c.id === 'settings') onSettings();
          else void c.run();
          if (!e.metaKey) onClose();
        } else if (q) {
          // free text -> search
          import('../../stores/viewStore').then(({ useView }) =>
            useView.getState().setView({ kind: 'search', q }),
          );
          onClose();
        }
      }
    };
    window.addEventListener('keydown', h, true);
    return () => window.removeEventListener('keydown', h, true);
  }, [results, hi, q, onClose, onCompose, onSettings]);

  useEffect(() => {
    const el = ref.current?.querySelector('input');
    el?.focus();
  }, []);

  return (
    <div style={{ position: 'fixed', inset: 0, background: 'rgb(0 0 0 / .2)', zIndex: 60 }} onClick={onClose}>
      <div
        ref={ref}
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 560,
          maxWidth: '92vw',
          margin: '15vh auto 0',
          background: 'var(--bg-elevated)',
          boxShadow: 'var(--shadow-dialog)',
          borderRadius: 8,
          overflow: 'hidden',
        }}
      >
        <Cmdk label="Command palette" shouldFilter={false}>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              padding: '10px 12px',
              borderBottom: '1px solid var(--border)',
            }}
          >
            <span style={{ color: 'var(--fg-3)', marginRight: 8 }}>⌘K</span>
            <Cmdk.Input
              value={q}
              onValueChange={setQ}
              placeholder="Type a command or search…"
              style={{
                flex: 1,
                border: 'none',
                outline: 'none',
                fontSize: 14,
                background: 'transparent',
                color: 'var(--fg)',
              }}
            />
          </div>
          <Cmdk.List style={{ maxHeight: 380, overflowY: 'auto', padding: 6 }}>
            {results.map((c, i) => (
              <Cmdk.Item
                key={c.id}
                value={c.id}
                onSelect={() => {
                  void c.run();
                  onClose();
                }}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  padding: '8px 10px',
                  borderRadius: 6,
                  background: i === hi ? 'var(--bg-row-focus)' : 'transparent',
                  fontSize: 13,
                  cursor: 'default',
                }}
                onMouseEnter={() => setHi(i)}
              >
                <span>
                  <span style={{ color: 'var(--fg-3)', fontSize: 11, marginRight: 8 }}>{c.section}</span>
                  {c.title}
                </span>
                {c.shortcut && <Kbd>{c.shortcut}</Kbd>}
              </Cmdk.Item>
            ))}
            {results.length === 0 && (
              <div style={{ padding: 16, fontSize: 13, color: 'var(--fg-3)' }}>
                No match — press ↩ to search for “{q}”
              </div>
            )}
          </Cmdk.List>
        </Cmdk>
      </div>
    </div>
  );
}
