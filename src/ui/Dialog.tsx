import React from 'react';
import { Dialog as BaseDialog } from '@base-ui-components/react/dialog';

export function Dialog({
  open,
  onClose,
  title,
  children,
  width = 480,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: React.ReactNode;
  width?: number;
}) {
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(o: boolean) => {
        if (!o) onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop
          style={{
            background: 'rgb(0 0 0 / .32)',
            position: 'fixed',
            inset: 0,
            transition: 'opacity 200ms var(--ease-out)',
          }}
        />
        <BaseDialog.Popup
          style={{
            position: 'fixed',
            top: '50%',
            left: '50%',
            transform: 'translate(-50%,-50%) scale(1)',
            transformOrigin: 'center',
            background: 'var(--bg-elevated)',
            boxShadow: 'var(--shadow-dialog)',
            borderRadius: 'var(--r-lg)',
            padding: 20,
            width,
            maxWidth: '90vw',
            transition: 'opacity 200ms var(--ease-out), transform 200ms var(--ease-out)',
          }}
        >
          <BaseDialog.Title style={{ fontSize: 14, fontWeight: 600, marginBottom: 12 }}>
            {title}
          </BaseDialog.Title>
          {children}
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
