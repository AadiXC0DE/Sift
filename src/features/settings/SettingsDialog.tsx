import React from 'react';
import { Dialog } from '../../ui/Dialog';
import { Segmented } from '../../ui/Segmented';
import { Switch } from '../../ui/Switch';
import { useSettings } from '../../stores/settingsStore';
import { useAccounts } from '../../stores/accountsStore';
import { api } from '../../app/ipc/commands';

const accents = ['blue', 'indigo', 'violet', 'rose', 'orange', 'green', 'teal', 'graphite'] as const;

export function SettingsDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const settings = useSettings((s) => s.settings);
  const set = useSettings((s) => s.set);
  const accounts = useAccounts((s) => s.accounts);
  const [tab, setTab] = React.useState('General');
  const tabs = ['General', 'Appearance', 'Accounts', 'Shortcuts', 'Notifications', 'Privacy', 'Advanced'];

  return (
    <Dialog open={open} onClose={onClose} title="Settings" width={760}>
      <div style={{ display: 'flex', gap: 16, minHeight: 420 }}>
        <nav style={{ width: 150, display: 'flex', flexDirection: 'column', gap: 2 }}>
          {tabs.map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              style={{
                textAlign: 'left',
                background: tab === t ? 'var(--bg-row-focus)' : 'none',
                border: 'none',
                borderRadius: 6,
                padding: '6px 10px',
                fontSize: 13,
                cursor: 'pointer',
              }}
            >
              {t}
            </button>
          ))}
        </nav>
        <div style={{ flex: 1, overflowY: 'auto', display: 'flex', flexDirection: 'column', gap: 16 }}>
          {tab === 'General' && (
            <>
              <Row label="Reading pane">
                <Segmented
                  value={settings.readingPane as never}
                  onChange={(v) => void set({ readingPane: v })}
                  options={[
                    { value: 'right', label: 'Right' },
                    { value: 'bottom', label: 'Bottom' },
                    { value: 'off', label: 'Off' },
                  ]}
                />
              </Row>
              <Row label="After archive, go to">
                <Segmented
                  value={settings.afterArchive as never}
                  onChange={(v) => void set({ afterArchive: v })}
                  options={[
                    { value: 'next', label: 'Next' },
                    { value: 'prev', label: 'Previous' },
                    { value: 'list', label: 'Back to list' },
                  ]}
                />
              </Row>
              <Row label="Undo send delay">
                <Segmented
                  value={String(settings.undoSendDelay) as never}
                  onChange={(v) => void set({ undoSendDelay: Number(v) })}
                  options={['0', '5', '10', '20', '30'].map((x) => ({ value: x, label: `${x}s` }))}
                />
              </Row>
              <Row label="Mark as read">
                <Segmented
                  value={settings.markAsRead as never}
                  onChange={(v) => void set({ markAsRead: v })}
                  options={[
                    { value: 'on-open', label: 'On open' },
                    { value: 'after-2s', label: 'After 2s' },
                    { value: 'manual', label: 'Manually' },
                  ]}
                />
              </Row>
              <Row label="Send and archive as default">
                <Switch
                  checked={settings.sendAndArchiveDefault}
                  onChange={(v) => void set({ sendAndArchiveDefault: v })}
                />
              </Row>
              <Row label="Split inbox by category">
                <Switch checked={settings.splitInbox} onChange={(v) => void set({ splitInbox: v })} />
              </Row>
              <button onClick={() => void api.sync_now()} style={{ alignSelf: 'flex-start' }}>
                Check for updates / Sync now
              </button>
              <button
                onClick={() => {
                  window.open('http://localhost:1420/__bench__', '_blank');
                }}
                style={{ display: 'none' }}
              >
                bench
              </button>
            </>
          )}
          {tab === 'Appearance' && (
            <>
              <Row label="Theme">
                <Segmented
                  value={settings.theme as never}
                  onChange={(v) => void set({ theme: v })}
                  options={[
                    { value: 'system', label: 'System' },
                    { value: 'light', label: 'Light' },
                    { value: 'dark', label: 'Dark' },
                  ]}
                />
              </Row>
              <Row label="Accent">
                <div style={{ display: 'flex', gap: 8 }}>
                  {accents.map((a) => (
                    <button
                      key={a}
                      onClick={() => void set({ accent: a })}
                      title={a}
                      style={{
                        width: 24,
                        height: 24,
                        borderRadius: '50%',
                        background: `var(--accent)`,
                        border: settings.accent === a ? '2px solid var(--fg)' : '2px solid transparent',
                        cursor: 'pointer',
                      }}
                      data-accent-swatch={a}
                    />
                  ))}
                </div>
              </Row>
              <Row label="UI font">
                <Segmented
                  value={settings.uiFont as never}
                  onChange={(v) => void set({ uiFont: v })}
                  options={[
                    { value: 'system', label: 'System' },
                    { value: 'inter', label: 'Inter' },
                    { value: 'geist', label: 'Geist' },
                  ]}
                />
              </Row>
              <Row label="Density">
                <Segmented
                  value={settings.density as never}
                  onChange={(v) => void set({ density: v })}
                  options={[
                    { value: 'compact', label: 'Compact' },
                    { value: 'default', label: 'Default' },
                    { value: 'comfortable', label: 'Comfortable' },
                  ]}
                />
              </Row>
              <Row label="Avatars in list">
                <Switch checked={settings.avatarsInList} onChange={(v) => void set({ avatarsInList: v })} />
              </Row>
              <Row label="Dark mode for emails">
                <Segmented
                  value={settings.darkModeEmails as never}
                  onChange={(v) => void set({ darkModeEmails: v })}
                  options={[
                    { value: 'auto', label: 'Auto' },
                    { value: 'always', label: 'Always' },
                    { value: 'never', label: 'Never' },
                  ]}
                />
              </Row>
            </>
          )}
          {tab === 'Accounts' && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
              {accounts.map((a) => (
                <div
                  key={a.id}
                  style={{
                    display: 'flex',
                    gap: 8,
                    alignItems: 'center',
                    border: '1px solid var(--border)',
                    borderRadius: 8,
                    padding: 8,
                  }}
                >
                  <span style={{ flex: 1, fontSize: 13 }}>{a.email}</span>
                  <input
                    value={a.color}
                    onChange={(e) => void api.accounts_update({ id: a.id, color: e.target.value })}
                    aria-label="color"
                    style={{ width: 80 }}
                  />
                  <button onClick={() => void api.accounts_remove(a.id)}>Remove</button>
                </div>
              ))}
              <button onClick={() => void api.accounts_add_google()}>Add account</button>
            </div>
          )}
          {tab === 'Shortcuts' && (
            <div style={{ fontSize: 13 }}>
              Presets: Sift / Gmail / Superhuman. Rebinding UI: press-to-record with conflict detection. (See
              ? overlay.)
            </div>
          )}
          {tab === 'Notifications' && (
            <>
              <Row label="New mail">
                <Segmented
                  value={settings.notifications as never}
                  onChange={(v) => void set({ notifications: v })}
                  options={[
                    { value: 'inbox', label: 'Inbox only' },
                    { value: 'everything', label: 'Everything' },
                    { value: 'off', label: 'Off' },
                  ]}
                />
              </Row>
              <Row label="Sound">
                <Segmented
                  value={settings.sound as never}
                  onChange={(v) => void set({ sound: v })}
                  options={[
                    { value: 'off', label: 'Off' },
                    { value: 'subtle', label: 'Subtle' },
                  ]}
                />
              </Row>
              <Row label="Dock badge">
                <Segmented
                  value={settings.dockBadge as never}
                  onChange={(v) => void set({ dockBadge: v })}
                  options={[
                    { value: 'unread', label: 'Unread in Inbox' },
                    { value: 'off', label: 'Off' },
                  ]}
                />
              </Row>
            </>
          )}
          {tab === 'Privacy' && (
            <>
              <Row label="Remote images">
                <Segmented
                  value={settings.remoteImages as never}
                  onChange={(v) => void set({ remoteImages: v })}
                  options={[
                    { value: 'never', label: 'Never' },
                    { value: 'ask', label: 'Ask' },
                    { value: 'always', label: 'Always' },
                  ]}
                />
              </Row>
              <Row label="Strip trackers">
                <Switch checked={settings.stripTrackers} onChange={(v) => void set({ stripTrackers: v })} />
              </Row>
            </>
          )}
          {tab === 'Advanced' && (
            <>
              <Row label="Offline body cache">
                <Segmented
                  value={settings.offlineBodyCache as never}
                  onChange={(v) => void set({ offlineBodyCache: v })}
                  options={[
                    { value: '6m', label: '6 months' },
                    { value: '2y', label: '2 years' },
                    { value: 'all', label: 'Everything' },
                  ]}
                />
              </Row>
              <Row label="Attachment cache">
                <Segmented
                  value={settings.attachmentCacheSize as never}
                  onChange={(v) => void set({ attachmentCacheSize: v })}
                  options={[
                    { value: '1GB', label: '1 GB' },
                    { value: '2GB', label: '2 GB' },
                    { value: '5GB', label: '5 GB' },
                  ]}
                />
              </Row>
              <button onClick={() => void api.diagnostics_export()}>Export diagnostics</button>
              <button
                onClick={() => {
                  if (window.confirm('Reset local data? Keeps Keychain.')) localStorage.clear();
                }}
              >
                Reset local data
              </button>
              <button
                onClick={() => {
                  const cmd = 'sift-bench quick';
                  try {
                    void navigator.clipboard.writeText(cmd);
                  } catch {
                    /* noop */
                  }
                }}
              >
                Compare speed with other mail apps… (copies: sift-bench quick)
              </button>
            </>
          )}
        </div>
      </div>
    </Dialog>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
      <span style={{ fontSize: 13 }}>{label}</span>
      {children}
    </div>
  );
}
