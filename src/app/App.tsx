import React, { useCallback, useEffect, useState } from 'react';
import { Sidebar } from '../features/sidebar/Sidebar';
import { ThreadList } from '../features/thread-list/ThreadList';
import { ThreadView } from '../features/thread-view/ThreadView';
import { Palette } from '../features/palette/Palette';
import { ComposerSheet } from '../features/compose/ComposerSheet';
import { SettingsDialog } from '../features/settings/SettingsDialog';
import { Onboarding } from '../features/onboarding/Onboarding';
import { ShortcutHelp } from '../features/palette/ShortcutHelp';
import { useView } from '../stores/viewStore';
import { useAccounts } from '../stores/accountsStore';
import { useSelection } from '../stores/selectionStore';
import { api } from './ipc/commands';
import { on } from './ipc/events';
import { useSync } from '../stores/syncStore';

export function App() {
  const paneLayout = useView((s) => s.paneLayout);
  const cyclePane = useView((s) => s.cyclePane);
  const threadId = useView((s) => s.threadId);
  const accounts = useAccounts((s) => s.accounts);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [composeOpen, setComposeOpen] = useState<null | { mode: string; threadId?: string }>(null);
  const [sidebarHidden, setSidebarHidden] = useState(false);
  const online = useSync((s) => s.online);

  const refreshAccounts = useAccounts((s) => s.refresh);

  useEffect(() => {
    const unsubs: (() => void)[] = [];
    on<{ account_id: string }>('auth:expired', () => {})
      .then((u) => unsubs.push(u))
      .catch(() => {});
    on('store:threads', () => {})
      .then((u) => unsubs.push(u))
      .catch(() => {});
    // global keys not in inputs
    const h = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setPaletteOpen((v) => !v);
        return;
      }
      if (typing) return;
      if (e.key === '?') setHelpOpen(true);
      if ((e.metaKey || e.ctrlKey) && e.key === ',') {
        e.preventDefault();
        setSettingsOpen(true);
      }
      if ((e.metaKey || e.ctrlKey) && e.key === '\\') setSidebarHidden((v) => !v);
      if (e.key === '`') cyclePane();
      if (e.key === 'c' && !e.metaKey && !e.ctrlKey) setComposeOpen({ mode: 'new' });
    };
    window.addEventListener('keydown', h);
    // first paint mark
    requestAnimationFrame(() => {
      api.perf_mark('first-paint').catch(() => {});
      api.perf_mark('webview-loaded').catch(() => {});
    });
    // kitchen sink route
    return () => {
      window.removeEventListener('keydown', h);
      unsubs.forEach((u) => u());
    };
  }, [cyclePane]);

  useEffect(() => {
    refreshAccounts();
  }, [refreshAccounts]);

  const showOnboarding = accounts.length === 0;

  if (window.location.hash === '#/kitchen-sink') {
    return <KitchenSink />;
  }

  return (
    <div className="sift-chrome" style={{ display: 'flex', height: '100vh', background: 'var(--bg-app)' }}>
      {!sidebarHidden && <Sidebar onSettings={() => setSettingsOpen(true)} />}
      <div
        style={{
          width: 'var(--list-w)',
          minWidth: 300,
          maxWidth: 560,
          borderRight: '1px solid var(--border)',
          display: 'flex',
          flexDirection: 'column',
          background: 'var(--bg-list)',
        }}
      >
        <div data-tauri-drag-region style={{ height: 38, flexShrink: 0 }} />
        {!online && (
          <div
            style={{
              height: 24,
              background: 'color-mix(in oklab, var(--warning) 14%, transparent)',
              color: 'var(--warning)',
              fontSize: 12,
              display: 'flex',
              alignItems: 'center',
              padding: '0 12px',
            }}
          >
            Offline · changes will sync when you&apos;re back
          </div>
        )}
        <ThreadList onCompose={() => setComposeOpen({ mode: 'new' })} />
      </div>
      {paneLayout !== 'off' || !threadId ? (
        <div
          style={{
            flex: 1,
            display: 'flex',
            flexDirection: paneLayout === 'bottom' ? 'column' : 'row',
            background: 'var(--bg-pane)',
            minWidth: 0,
          }}
        >
          <ThreadView onReply={(mode, tid) => setComposeOpen({ mode, threadId: tid })} />
        </div>
      ) : null}
      {showOnboarding && <Onboarding />}
      {paletteOpen && (
        <Palette
          onClose={() => setPaletteOpen(false)}
          onCompose={() => setComposeOpen({ mode: 'new' })}
          onSettings={() => setSettingsOpen(true)}
        />
      )}
      {composeOpen && (
        <ComposerSheet
          mode={composeOpen.mode}
          threadId={composeOpen.threadId}
          onClose={() => setComposeOpen(null)}
        />
      )}
      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />
      <ShortcutHelp open={helpOpen} onClose={() => setHelpOpen(false)} />
      <SeqHint />
      <LearnKeys />
    </div>
  );
}

function SeqHint() {
  const [seq, setSeq] = useState<string | null>(null);
  useEffect(() => {
    import('../keymap/engine').then(({ engine }) => {
      engine.pendingHint = setSeq;
    });
  }, []);
  if (!seq) return null;
  return (
    <div
      style={{
        position: 'fixed',
        bottom: 12,
        left: 12,
        background: 'var(--n10)',
        color: 'var(--n0)',
        fontSize: 12,
        padding: '4px 8px',
        borderRadius: 6,
      }}
    >
      {seq} …
    </div>
  );
}

function LearnKeys() {
  const [dismissed, setDismissed] = useState(() => localStorage.getItem('sift-learn-dismissed') === '1');
  const setFocus = useSelection((s) => s.setFocus);
  void setFocus;
  const dismiss = useCallback(() => {
    localStorage.setItem('sift-learn-dismissed', '1');
    setDismissed(true);
  }, []);
  useEffect(() => {
    void dismiss;
  }, [dismiss]);
  if (dismissed) return null;
  return (
    <div
      style={{
        position: 'fixed',
        bottom: 0,
        left: 'var(--sidebar-w)',
        right: 0,
        display: 'flex',
        gap: 16,
        justifyContent: 'center',
        padding: '6px 12px',
        fontSize: 12,
        color: 'var(--fg-3)',
        background: 'var(--bg-list)',
        borderTop: '1px solid var(--border)',
      }}
    >
      <span>
        <b>j/k</b> move
      </span>
      <span>
        <b>e</b> archive
      </span>
      <span>
        <b>⌘K</b> anything
      </span>
      <button
        onClick={dismiss}
        style={{ background: 'none', border: 'none', color: 'var(--fg-3)', cursor: 'pointer' }}
      >
        Dismiss
      </button>
    </div>
  );
}

function KitchenSink() {
  const [theme, setTheme] = useState('light');
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  return (
    <div style={{ padding: 32, display: 'flex', flexDirection: 'column', gap: 16 }}>
      <h1>Kitchen sink ({theme})</h1>
      <button onClick={() => setTheme(theme === 'light' ? 'dark' : 'light')}>Toggle theme</button>
      <div id="ks-buttons" style={{ display: 'flex', gap: 8 }}>
        <button className="sift-btn-primary">Primary</button>
        <button>Secondary</button>
      </div>
    </div>
  );
}
