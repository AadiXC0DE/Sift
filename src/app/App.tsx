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
import { api } from './ipc/commands';
import { on } from './ipc/events';
import { useSync } from '../stores/syncStore';
import { engine } from '../keymap/engine';
import { undoLast } from '../features/actions/dispatch';
import { Kbd } from '../ui/Kbd';
import { Button } from '../ui/Button';

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
  const setThread = useView((s) => s.setThread);

  // Engine owns sequences (g i …), go-tos and global undo; single-key
  // shortcuts live with their feature handlers to avoid double dispatch.
  useEffect(() => {
    engine.onAction = (action) => {
      const v = useView.getState();
      if (action === 'goInbox') v.setView({ kind: 'inbox' });
      else if (action === 'goStarred') v.setView({ kind: 'starred' });
      else if (action === 'goSnoozed') v.setView({ kind: 'snoozed' });
      else if (action === 'goSent') v.setView({ kind: 'sent' });
      else if (action === 'goDrafts') v.setView({ kind: 'drafts' });
      else if (action === 'goArchive') v.setView({ kind: 'archive' });
      else if (action === 'undo') void undoLast();
    };
  }, []);

  useEffect(() => {
    const unsubs: (() => void)[] = [];
    on<{ account_id: string }>('auth:expired', () => {})
      .then((u) => unsubs.push(u))
      .catch(() => {});
    on('store:threads', () => {})
      .then((u) => unsubs.push(u))
      .catch(() => {});
    // Screenshot-pipeline driver (demo builds only emit demo:goto).
    on<{ scene: string }>('demo:goto', (p) => {
      const scene = (p as unknown as { scene: string }).scene;
      if (scene === 'setup' || scene.startsWith('setup')) {
        const step =
          scene === 'setup-email'
            ? 'email'
            : scene === 'setup-app-password'
              ? 'app-password'
              : scene === 'setup-connecting'
                ? 'connecting'
                : 'welcome';
        import('../stores/setupStore').then(({ useSetup }) =>
          useSetup.setState({
            step: step as 'welcome' | 'email' | 'app-password' | 'connecting',
            email: 'you@gmail.com',
            demoForce: true,
          }),
        );
        return;
      }
      const view = useView.getState();
      if (scene === 'palette') setPaletteOpen(true);
      else if (scene === 'compose') setComposeOpen({ mode: 'new' });
      else if (scene === 'settings') setSettingsOpen(true);
      else if (scene === 'search') view.setView({ kind: 'search', q: 'invoice' });
      else {
        view.setScope('all');
        view.setView({ kind: 'inbox' });
      }
    })
      .then((u) => unsubs.push(u))
      .catch(() => {});
    // Dock badge = unread inbox threads across included accounts.
    on('store:labels', () => {
      void (async () => {
        try {
          const accs = useAccounts.getState();
          let total = 0;
          for (const a of accs.accounts) {
            if (accs.included[a.id] === false) continue;
            const labels = await api.labels_list(a.id);
            const inbox = labels.find((l) => l.id === 'INBOX');
            total += inbox?.unread_count ?? 0;
          }
          await api.app_set_badge(total);
        } catch {
          /* offline */
        }
      })();
    })
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
      // Sequences + global undo first (engine ignores text inputs itself).
      if (engine.handle(e, ['global'])) return;
      if (typing) return;
      // Back out of a full-width thread to the list.
      if (
        (e.key === 'Escape' || e.key === 'u') &&
        !paletteOpen &&
        !composeOpen &&
        !settingsOpen &&
        !helpOpen &&
        useView.getState().paneLayout === 'off' &&
        useView.getState().threadId != null
      ) {
        e.preventDefault();
        setThread(null);
        return;
      }
      if (e.key === '?') setHelpOpen(true);
      if ((e.metaKey || e.ctrlKey) && e.key === ',') {
        e.preventDefault();
        setSettingsOpen(true);
      }
      if ((e.metaKey || e.ctrlKey) && e.key === '\\') setSidebarHidden((v) => !v);
      if (e.key === '`') cyclePane();
      if (e.key === 'c' && !e.metaKey && !e.ctrlKey) setComposeOpen({ mode: 'new' });
    };
    // Overlay Escape must beat WKWebView/macOS (which otherwise miniaturizes).
    const onEsc = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (composeOpen) {
        e.preventDefault();
        e.stopPropagation();
        setComposeOpen(null);
        return;
      }
      if (paletteOpen) {
        e.preventDefault();
        e.stopPropagation();
        setPaletteOpen(false);
        return;
      }
      if (settingsOpen) {
        e.preventDefault();
        e.stopPropagation();
        setSettingsOpen(false);
        return;
      }
      if (helpOpen) {
        e.preventDefault();
        e.stopPropagation();
        setHelpOpen(false);
      }
    };
    window.addEventListener('keydown', onEsc, true);
    window.addEventListener('keydown', h);
    // first paint mark
    requestAnimationFrame(() => {
      api.perf_mark('first-paint').catch(() => {});
      api.perf_mark('webview-loaded').catch(() => {});
    });
    // kitchen sink route
    return () => {
      window.removeEventListener('keydown', onEsc, true);
      window.removeEventListener('keydown', h);
      unsubs.forEach((u) => u());
    };
  }, [cyclePane, paletteOpen, composeOpen, settingsOpen, helpOpen, setThread]);

  useEffect(() => {
    refreshAccounts();
  }, [refreshAccounts]);

  const showOnboarding = accounts.length === 0;
  const paneOffOpen = paneLayout === 'off' && threadId != null;
  const bottom = paneLayout === 'bottom';
  const showList = !paneOffOpen;
  const showThread = paneLayout !== 'off' || threadId != null;

  if (window.location.hash === '#/kitchen-sink') {
    return <KitchenSink />;
  }

  return (
    <div
      className="sift-chrome"
      style={{
        display: 'flex',
        flexDirection: 'column',
        height: '100vh',
        background: 'var(--bg-app)',
        overflow: 'hidden',
        minWidth: 0,
      }}
    >
      <div data-tauri-drag-region style={{ height: 28, flexShrink: 0 }} />
      <div style={{ display: 'flex', flex: 1, minHeight: 0, minWidth: 0, overflow: 'hidden' }}>
        {!sidebarHidden && <Sidebar onSettings={() => setSettingsOpen(true)} />}
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            flex: 1,
            minWidth: 0,
            minHeight: 0,
            overflow: 'hidden',
          }}
        >
          <div
            style={{
              display: 'flex',
              flexDirection: bottom ? 'column' : 'row',
              flex: 1,
              minWidth: 0,
              minHeight: 0,
              overflow: 'hidden',
            }}
          >
            {showList && (
              <div
                style={{
                  width: bottom ? 'auto' : 'var(--list-w)',
                  flex: bottom ? '0 0 38%' : '0 1 var(--list-w)',
                  minWidth: 0,
                  minHeight: 0,
                  maxWidth: bottom ? 'none' : 560,
                  borderRight: bottom ? 'none' : '1px solid var(--border)',
                  borderBottom: bottom ? '1px solid var(--border)' : 'none',
                  display: 'flex',
                  flexDirection: 'column',
                  background: 'var(--bg-list)',
                  overflow: 'hidden',
                }}
              >
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
                      flexShrink: 0,
                    }}
                  >
                    Offline · changes will sync when you&apos;re back
                  </div>
                )}
                <ThreadList onCompose={() => setComposeOpen({ mode: 'new' })} />
              </div>
            )}
            {showThread && (
              <div
                style={{
                  flex: 1,
                  display: 'flex',
                  minWidth: 0,
                  minHeight: 0,
                  overflow: 'hidden',
                  background: 'var(--bg-pane)',
                }}
              >
                <ThreadView onReply={(mode, tid) => setComposeOpen({ mode, threadId: tid })} />
              </div>
            )}
          </div>
          <LearnKeys />
        </div>
      </div>
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
  const [dismissed, setDismissed] = useState(() => sessionStorage.getItem('sift-learn-dismissed') === '1');
  const dismiss = useCallback(() => {
    sessionStorage.setItem('sift-learn-dismissed', '1');
    setDismissed(true);
  }, []);
  if (dismissed) return null;
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 8,
        height: 26,
        padding: '0 10px',
        fontSize: 11,
        color: 'var(--fg-3)',
        background: 'var(--bg-list)',
        borderTop: '1px solid var(--border)',
        flexShrink: 0,
      }}
    >
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
        <Kbd>j</Kbd>
        <Kbd>k</Kbd>
        move
      </span>
      <span style={{ opacity: 0.4 }}>·</span>
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
        <Kbd>e</Kbd>
        archive
      </span>
      <span style={{ opacity: 0.4 }}>·</span>
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
        <Kbd>r</Kbd>
        reply
      </span>
      <span style={{ opacity: 0.4 }}>·</span>
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
        <Kbd>c</Kbd>
        compose
      </span>
      <span style={{ opacity: 0.4 }}>·</span>
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
        <Kbd>⌘K</Kbd>
        anything
      </span>
      <button
        onClick={dismiss}
        style={{
          marginLeft: 8,
          background: 'none',
          border: 'none',
          color: 'var(--fg-3)',
          cursor: 'pointer',
          fontSize: 11,
          padding: 0,
        }}
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
      <Button onClick={() => setTheme(theme === 'light' ? 'dark' : 'light')}>Toggle theme</Button>
      <div id="ks-buttons" style={{ display: 'flex', gap: 8 }}>
        <Button variant="primary">Primary</Button>
        <Button>Secondary</Button>
      </div>
    </div>
  );
}
