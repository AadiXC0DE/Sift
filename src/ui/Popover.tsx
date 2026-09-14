import React, { useEffect, useState } from 'react';
import { Popover as BasePopover } from '@base-ui-components/react/popover';
import { pushSurface } from './overlayStack';

export function Popover({
  trigger,
  children,
  label,
  open,
  onOpenChange,
}: {
  trigger: React.ReactElement;
  children: React.ReactNode;
  label?: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  void label;
  // `open` stays optional: when it is omitted the popover is uncontrolled, so
  // the component tracks its own open state to report it to the surface stack.
  const [selfOpen, setSelfOpen] = useState(false);
  const isOpen = open ?? selfOpen;
  useEffect(() => {
    if (!isOpen) return;
    // A popover owns Escape while it is open: the app-level handler must not
    // close the surface underneath it (P9.5).
    return pushSurface({ kind: 'native' });
  }, [isOpen]);
  return (
    <BasePopover.Root
      open={open}
      onOpenChange={(next: boolean) => {
        setSelfOpen(next);
        onOpenChange?.(next);
      }}
    >
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
