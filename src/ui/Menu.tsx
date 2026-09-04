import React from 'react';
import { Menu as BaseMenu } from '@base-ui-components/react/menu';
import { Kbd } from './Kbd';

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
  return (
    <BaseMenu.Root>
      <BaseMenu.Trigger render={trigger as never} />
      <BaseMenu.Portal>
        <BaseMenu.Positioner sideOffset={4}>
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
