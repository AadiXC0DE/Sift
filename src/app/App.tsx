import React, { Suspense, useCallback, useEffect, useRef, useState } from 'react';
import { Sidebar } from '../features/sidebar/Sidebar';
import { ThreadList } from '../features/thread-list/ThreadList';
import { ThreadView } from '../features/thread-view/ThreadView';
import { Onboarding } from '../features/onboarding/Onboarding';
import { onIdle } from '../lib/idle';
import { useView } from '../stores/viewStore';
import { useAccounts } from '../stores/accountsStore';
import { api } from './ipc/commands';
import { on } from './ipc/events';
import { useSync } from '../stores/syncStore';
import type { SyncStatus, ThreadRef } from './ipc/types';
import { engine } from '../keymap/engine';
import { activeKeyScopes } from '../keymap/scopes';
import { undoLast } from '../features/actions/dispatch';
import { Kbd } from '../ui/Kbd';
import { Button } from '../ui/Button';

// Overlays and optional utilities stay out of the startup bundle: TipTap, the
// command palette and the settings panes must not be parsed or downloaded
// before the inbox and reader have painted. `manualChunks` plus a dynamic
// import keeps each in its own lazily requested chunk.
const Palette = React.lazy(() => import('../features/palette/Palette').then((m) => ({ default: m.Palette })));
const ComposerSheet = React.lazy(() =>
  import('../features/compose/ComposerSheet').then((m) => ({ default: m.ComposerSheet })),
);
const SettingsDialog = React.lazy(() =>
  import('../features/settings/SettingsDialog').then((m) => ({ default: m.SettingsDialog })),
);
const ShortcutHelp = React.lazy(() =>
  import('../features/palette/ShortcutHelp').then((m) => ({ default: m.ShortcutHelp })),
);

export function App() {
  const paneLayout = useView((s) => s.paneLayout);
  const cyclePane = useView((s) => s.cyclePane);
  const openThread = useView((s) => s.openThread);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [addAccountOpen, setAddAccountOpen] = useState(false);
  const [composeOpen, setComposeOpen] = useState<null | { mode: string; thread?: ThreadRef }>(null);
  const [sidebarHidden, setSidebarHidden] = useState(false);
  // Lazy overlays mount on first open and stay mounted so their close
  // transitions still run; they are never parsed before the user asks.
  const [settingsMounted, setSettingsMounted] = useState(false);
  const [helpMounted, setHelpMounted] = useState(false);
  const prefetchedComposer = useRef(false);
  const online = useSync((s) => s.online);
  const accountsReady = useAccounts((s) => s.accounts.length > 0);

  const refreshAccounts = useAccounts((s) => s.refresh);
  const setOpenThread = useView((s) => s.setOpenThread);
  // Any overlay covers the reader: its mark-read timer must not fire behind it.
  const modalOpen = paletteOpen || composeOpen != null || settingsOpen || helpOpen || addAccountOpen;

  useEffect(() => {
    if (settingsOpen) setSettingsMounted(true);
  }, [settingsOpen]);
  useEffect(() => {
    if (helpOpen) setHelpMounted(true);
  }, [helpOpen]);

  // Warm the composer once the inbox has something to show and the engine is
  // idle, so `c` is instant without paying TipTap's parse cost at launch.
  // First-run onboarding owns the screen instead; it prefetches when the
  // account list arrives.
  useEffect(() => {
    if (!accountsReady || prefetchedComposer.current) return;
    prefetchedComposer.current = true;
    return onIdle(() => {
      void import('../features/compose/ComposerSheet');
    });
  }, [accountsReady]);

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
      // Modal scopes win: while an overlay is open the list/thread scopes are
      // not active, so a keystroke cannot trigger a hidden mail action.
      const scopes = activeKeyScopes({
        paletteOpen,
        modalOpen: composeOpen != null || settingsOpen || helpOpen || addAccountOpen,
        threadOpen: useView.getState().openThread != null,
      });
      // Sequences + scoped bindings first (engine ignores text inputs itself).
      if (engine.handle(e, scopes)) return;
      if (typing) return;
      // Back out of a full-width thread to the list.
      if (
        (e.key === 'Escape' || e.key === 'u') &&
        !paletteOpen &&
        !composeOpen &&
        !settingsOpen &&
        !helpOpen &&
        useView.getState().paneLayout === 'off' &&
        useView.getState().openThread != null
      ) {
        e.preventDefault();
        setOpenThread(null);
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
        // The composer owns Escape while one of its dismissible layers (for
        // example the recipient suggestion list) is open: that layer closes
        // itself and clears the attribute, so the next Escape closes the sheet.
        if (document.querySelector('[data-compose-escape="1"]')) return;
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
    // Capture so the list/thread bindings beat in-page typeahead helpers, as
    // the feature-level listeners used to.
    window.addEventListener('keydown', h, true);
    // first paint mark
    requestAnimationFrame(() => {
      api.perf_mark('first-paint').catch(() => {});
      api.perf_mark('webview-loaded').catch(() => {});
    });
    // kitchen sink route
    return () => {
      window.removeEventListener('keydown', onEsc, true);
      window.removeEventListener('keydown', h, true);
      unsubs.forEach((u) => u());
    };
  }, [cyclePane, paletteOpen, composeOpen, settingsOpen, helpOpen, addAccountOpen, setOpenThread]);

  useEffect(() => {
    refreshAccounts();
  }, [refreshAccounts]);

  // Sync progress, outbox depth, and connectivity feed the first-run UI and
  // the sidebar status. Without this the mailbox looked empty during the
  // initial download with no feedback.
  useEffect(() => {
    const unsubs: (() => void)[] = [];
    on<SyncStatus>('sync:state', (s) => {
      useSync.getState().setStatus(s);
      if (s.phase === 'done' || s.phase === 'error') void useAccounts.getState().refresh();
    })
      .then((u) => unsubs.push(u))
      .catch(() => {});
    on<{ account_id: string; pending: number; summary?: { label: string; count: number }[] }>(
      'outbox:state',
      (p) => {
        useSync.getState().setPending(p.account_id, p.pending, p.summary ?? []);
      },
    )
      .then((u) => unsubs.push(u))
      .catch(() => {});
    const setOnline = () => useSync.getState().setOnline(navigator.onLine);
    setOnline();
    window.addEventListener('online', setOnline);
    window.addEventListener('offline', setOnline);
    return () => {
      unsubs.forEach((u) => u());
      window.removeEventListener('online', setOnline);
      window.removeEventListener('offline', setOnline);
    };
  }, []);

  const paneOffOpen = paneLayout === 'off' && openThread != null;
  const bottom = paneLayout === 'bottom';
  const showList = !paneOffOpen;
  const showThread = paneLayout !== 'off' || openThread != null;

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
                data-tauri-drag-region
                style={{
                  width: bottom ? 'auto' : 'var(--list-w)',
                  flex: bottom ? '0 0 38%' : '0 1 var(--list-w)',
                  minWidth: 0,
                  minHeight: 0,
                  paddingTop: 'var(--titlebar-h)',
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
                data-tauri-drag-region
                style={{
                  flex: 1,
                  display: 'flex',
                  minWidth: 0,
                  minHeight: 0,
                  paddingTop: 'var(--titlebar-h)',
                  overflow: 'hidden',
                  background: 'var(--bg-pane)',
                }}
              >
                <ThreadView
                  obscured={modalOpen}
                  onReply={(mode, thread) => setComposeOpen({ mode, thread })}
                />
              </div>
            )}
          </div>
          <LearnKeys />
        </div>
      </div>
      <Onboarding force={addAccountOpen} onClose={() => setAddAccountOpen(false)} />
      {paletteOpen && (
        <Suspense fallback={null}>
          <Palette
            onClose={() => setPaletteOpen(false)}
            onCompose={() => setComposeOpen({ mode: 'new' })}
            onSettings={() => setSettingsOpen(true)}
          />
        </Suspense>
      )}
      {composeOpen && (
        <Suspense fallback={null}>
          <ComposerSheet
            mode={composeOpen.mode}
            thread={composeOpen.thread}
            onClose={() => setComposeOpen(null)}
          />
        </Suspense>
      )}
      {settingsMounted && (
        <Suspense fallback={null}>
          <SettingsDialog
            open={settingsOpen}
            onClose={() => setSettingsOpen(false)}
            onAddAccount={() => {
              setSettingsOpen(false);
              setAddAccountOpen(true);
            }}
          />
        </Suspense>
      )}
      {helpMounted && (
        <Suspense fallback={null}>
          <ShortcutHelp open={helpOpen} onClose={() => setHelpOpen(false)} />
        </Suspense>
      )}
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
