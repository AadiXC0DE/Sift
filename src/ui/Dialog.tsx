import React from 'react';
import { Dialog as BaseDialog } from '@base-ui-components/react/dialog';
import { X } from 'lucide-react';

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
            maxHeight: '86vh',
            overflow: 'auto',
            color: 'var(--fg)',
            transition: 'opacity 200ms var(--ease-out), transform 200ms var(--ease-out)',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 12 }}>
            <BaseDialog.Title style={{ fontSize: 14, fontWeight: 600, flex: 1, margin: 0 }}>
              {title}
            </BaseDialog.Title>
            <button
              type="button"
              aria-label="Close"
              onClick={onClose}
              className="sift-iconbtn"
              style={{
                width: 28,
                height: 28,
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                borderRadius: 'var(--r-md)',
                border: '1px solid transparent',
                background: 'transparent',
                color: 'var(--fg-2)',
                cursor: 'pointer',
              }}
            >
              <X size={16} />
            </button>
          </div>
          {children}
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
