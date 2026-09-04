import React from 'react';
import { Popover as BasePopover } from '@base-ui-components/react/popover';

export function Popover({
  trigger,
  children,
  label,
}: {
  trigger: React.ReactElement;
  children: React.ReactNode;
  label?: string;
}) {
  void label;
  return (
    <BasePopover.Root>
      <BasePopover.Trigger render={trigger as never} />
      <BasePopover.Portal>
        <BasePopover.Positioner sideOffset={6}>
          <BasePopover.Popup
            style={{
              background: 'var(--bg-elevated)',
              boxShadow: 'var(--shadow-popover)',
              borderRadius: 'var(--r-md)',
              padding: 8,
              transformOrigin: 'var(--transform-origin)',
              transition: 'opacity 160ms var(--ease-out), transform 160ms var(--ease-out)',
            }}
          >
            {children}
          </BasePopover.Popup>
        </BasePopover.Positioner>
      </BasePopover.Portal>
    </BasePopover.Root>
  );
}
