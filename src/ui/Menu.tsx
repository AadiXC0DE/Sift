import React, { useEffect, useState } from 'react';
import { Menu as BaseMenu } from '@base-ui-components/react/menu';
import { Kbd } from './Kbd';
import { pushSurface } from './overlayStack';

export interface MenuItem {
  label: string;
  hint?: string;
  action: () => void;
  danger?: boolean;
}

export function Menu({
  trigger,
  items,
  label,
}: {
  trigger: React.ReactElement;
  items: MenuItem[];
  label?: string;
}) {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return;
    // While a menu is open the list/reader shortcuts must not fire, and Escape
    // belongs to the menu (P9.5).
    return pushSurface({ kind: 'native' });
  }, [open]);
  return (
    <BaseMenu.Root onOpenChange={(next: boolean) => setOpen(next)}>
      <BaseMenu.Trigger render={trigger as never} />
      <BaseMenu.Portal>
        {/*
          The portal lands in a static container at the end of `<body>`, so a
          positioned popup only outranks in-app content by DOM order. The
          composer sheet is the one app surface that claims a stacking layer of
          its own (`z-index: 50` in Sheet.tsx), which would otherwise paint over
          every menu opened from inside it — including the Send Later menu.
        */}
        <BaseMenu.Positioner sideOffset={4} style={{ zIndex: 60 }}>
          <BaseMenu.Popup
            style={{
              background: 'var(--bg-elevated)',
              boxShadow: 'var(--shadow-popover)',
              borderRadius: 'var(--r-md)',
              padding: 4,
              minWidth: 200,
              transformOrigin: 'var(--transform-origin)',
              transition: 'opacity 160ms var(--ease-out), transform 160ms var(--ease-out)',
            }}
          >
            {label && <div style={{ fontSize: 11, color: 'var(--fg-3)', padding: '4px 8px' }}>{label}</div>}
            {items.map((it, i) => (
              <BaseMenu.Item
                key={i}
                onClick={it.action}
                style={{
                  height: 28,
                  fontSize: 13,
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  padding: '0 8px',
                  borderRadius: 5,
                  cursor: 'default',
                  color: it.danger ? 'var(--danger)' : 'var(--fg)',
                }}
              >
                <span>{it.label}</span>
                {it.hint && <Kbd>{it.hint}</Kbd>}
              </BaseMenu.Item>
            ))}
          </BaseMenu.Popup>
        </BaseMenu.Positioner>
      </BaseMenu.Portal>
    </BaseMenu.Root>
  );
}

export function ContextMenu({ children, items }: { children: React.ReactElement; items: MenuItem[] }) {
  void children;
  return <Menu trigger={<button style={{ display: 'none' }} />} items={items} />;
}
