import React, { useEffect } from 'react';
import { Toaster } from 'sonner';
import { Tooltip } from '@base-ui-components/react/tooltip';
import { useSettings } from '../stores/settingsStore';
import { useAccounts } from '../stores/accountsStore';
import { engine } from '../keymap/engine';
import { defaultBindings } from '../keymap/defaults';

export function Providers({ children }: { children: React.ReactNode }) {
  const load = useSettings((s) => s.load);
  const refresh = useAccounts((s) => s.refresh);
  useEffect(() => {
    // The engine owns only multi-key sequences (g i …), go-tos and global
    // undo. Single-key shortcuts live with their feature handlers so one
    // keystroke never dispatches twice.
    engine.register(
      defaultBindings.filter((b) => b.key.includes(' ') || b.action === 'undo' || b.action.startsWith('go')),
    );
    load();
    refresh();
    // matchMedia listener for system theme
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const h = () => {
      const st = useSettings.getState().settings;
      if (st.theme === 'system') {
        document.documentElement.dataset.theme = mq.matches ? 'dark' : 'light';
      }
    };
    mq.addEventListener('change', h);
    return () => mq.removeEventListener('change', h);
  }, [load, refresh]);
  return (
    <>
      <Tooltip.Provider>{children}</Tooltip.Provider>
      <Toaster
        position="bottom-right"
        toastOptions={{
          style: { background: 'var(--bg-elevated)', color: 'var(--fg)', border: '1px solid var(--border)' },
        }}
      />
    </>
  );
}
