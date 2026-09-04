import React, { useRef, useState } from 'react';
import { Tooltip as BaseTooltip } from '@base-ui-components/react/tooltip';

let lastClose = 0;

export function Tooltip({ content, children }: { content: string; children: React.ReactElement }) {
  const [instant, setInstant] = useState(false);
  const t = useRef(0);
  return (
    <BaseTooltip.Provider delay={400}>
      <BaseTooltip.Root
        onOpenChange={(open: boolean) => {
          if (open) setInstant(Date.now() - lastClose < 300);
          else lastClose = Date.now();
        }}
      >
        <BaseTooltip.Trigger render={children as never} />
        <BaseTooltip.Portal>
          <BaseTooltip.Positioner sideOffset={6}>
            <BaseTooltip.Popup
              data-instant={instant || undefined}
              ref={(el: HTMLElement | null) => {
                if (el) t.current = Date.now();
                void t;
              }}
              style={{
                background: 'var(--n10)',
                color: 'var(--n0)',
                fontSize: 12,
                padding: '4px 8px',
                borderRadius: 'var(--r-sm)',
                transformOrigin: 'var(--transform-origin)',
                transition: instant
                  ? 'none'
                  : 'opacity 125ms var(--ease-out), transform 125ms var(--ease-out)',
              }}
            >
              {content}
            </BaseTooltip.Popup>
          </BaseTooltip.Positioner>
        </BaseTooltip.Portal>
      </BaseTooltip.Root>
    </BaseTooltip.Provider>
  );
}
