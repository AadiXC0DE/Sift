import React, { useEffect } from 'react';
import { Toaster } from 'sonner';
import { Tooltip } from '@base-ui-components/react/tooltip';
import { useSettings } from '../stores/settingsStore';
import { useAccounts } from '../stores/accountsStore';
import { useUpdates } from '../features/updates/updatesStore';
import { engine } from '../keymap/engine';
import { defaultBindings } from '../keymap/defaults';

export function Providers({ children }: { children: React.ReactNode }) {
  const load = useSettings((s) => s.load);
  const refresh = useAccounts((s) => s.refresh);
  useEffect(() => {
    // The engine resolves bindings for the active scopes; feature components
    // subscribe by action name (useKeymap), so list/thread bindings participate
    // in remapping instead of being hardcoded in each feature's key handler.
    engine.register(
      defaultBindings.filter(
        (b) =>
          b.key.includes(' ') ||
          b.action === 'undo' ||
          b.action.startsWith('go') ||
          b.scope === 'list' ||
          b.scope === 'thread',
      ),
    );
    refresh();
    /*
     * The one automatic update check (P11.1) rides on the settings load rather
     * than racing it: the user's choice is in hand before anything is checked,
     * it happens at most once per launch, and it is skipped while offline. It
     * reads the release manifest and stops there — downloading and installing
     * are the user's, from Settings -> Updates.
     */
    void load().then(() => useUpdates.getState().check({ automatic: true }));
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
