import React from 'react';
import { Dialog } from '../../ui/Dialog';
import { Segmented } from '../../ui/Segmented';
import { Switch } from '../../ui/Switch';
import { useSettings } from '../../stores/settingsStore';
import { useAccounts } from '../../stores/accountsStore';
import { api } from '../../app/ipc/commands';
import { Button } from '../../ui/Button';
import { Kbd } from '../../ui/Kbd';
import { defaultBindings } from '../../keymap/defaults';

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
                color: tab === t ? 'var(--fg)' : 'var(--fg-2)',
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
              <Button onClick={() => void api.sync_now()} style={{ alignSelf: 'flex-start' }}>
                Sync now
              </Button>
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
                        background: `var(--swatch-${a})`,
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
                    padding: '10px 12px',
                    background: 'var(--n1)',
                  }}
                >
                  <span
                    style={{
                      flex: 1,
                      fontSize: 13,
                      minWidth: 0,
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                    }}
                  >
                    {a.email}
                  </span>
                  <select
                    aria-label="Account color"
                    value={a.color}
                    onChange={(e) => void api.accounts_update({ id: a.id, color: e.target.value })}
                    style={{
                      height: 28,
                      border: '1px solid var(--border-strong)',
                      borderRadius: 6,
                      padding: '0 8px',
                      background: 'var(--bg-raised)',
                      color: 'var(--fg)',
                    }}
                  >
                    {accents.map((c) => (
                      <option key={c} value={c}>
                        {c}
                      </option>
                    ))}
                  </select>
                  <Button size="sm" variant="danger" onClick={() => void api.accounts_remove(a.id)}>
                    Remove
                  </Button>
                </div>
              ))}
              <Button onClick={() => void api.accounts_add_google()} style={{ alignSelf: 'flex-start' }}>
                Add account
              </Button>
            </div>
          )}
          {tab === 'Shortcuts' && (
            <div
              style={{ display: 'flex', flexDirection: 'column', gap: 4, maxHeight: 360, overflowY: 'auto' }}
            >
              {defaultBindings.slice(0, 24).map((b, i) => (
                <div
                  key={`${b.scope}:${b.key}:${i}`}
                  style={{
                    display: 'flex',
                    justifyContent: 'space-between',
                    gap: 12,
                    fontSize: 13,
                    padding: '4px 0',
                  }}
                >
                  <span style={{ color: 'var(--fg-2)' }}>{b.action}</span>
                  <Kbd>{b.key}</Kbd>
                </div>
              ))}
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
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8, alignItems: 'flex-start' }}>
                <Button onClick={() => void api.diagnostics_export()}>Export diagnostics</Button>
                <Button
                  variant="danger"
                  onClick={() => {
                    if (window.confirm('Reset local data? Keeps Keychain.')) localStorage.clear();
                  }}
                >
                  Reset local data
                </Button>
                <Button
                  onClick={() => {
                    try {
                      void navigator.clipboard.writeText('sift-bench quick');
                    } catch {
                      /* noop */
                    }
                  }}
                >
                  Copy speed-bench command
                </Button>
              </div>
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
