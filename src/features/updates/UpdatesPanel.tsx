import React from 'react';
import { useSettings } from '../../stores/settingsStore';
import { useUpdates } from './updatesStore';
import { Button } from '../../ui/Button';
import { Switch } from '../../ui/Switch';
import { relativeTime } from '../../lib/dates';
import {
  describeLastCheck,
  downloadPercent,
  formatReleaseDate,
  type LastCheck,
  type UpdateFailure,
} from './updates';

const MUTED: React.CSSProperties = { fontSize: 12, color: 'var(--fg-2)' };

/**
 * The exact cause, in the plugin's own words. Kept small and secondary: the
 * message above it is what the user acts on, this is what a bug report needs.
 */
function FailureDetail({ failure }: { failure: UpdateFailure }) {
  return (
    <div data-testid="updates-detail" style={{ ...MUTED, wordBreak: 'break-word' }}>
      {failure.code}: {failure.detail}
    </div>
  );
}

function FailureBlock({ failure }: { failure: UpdateFailure }) {
  return (
    <div
      data-testid="updates-failure"
      data-code={failure.code}
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 4,
        border: '1px solid var(--border)',
        borderRadius: 8,
        padding: '10px 12px',
        background: 'var(--n1)',
      }}
    >
      <span style={{ fontSize: 13 }}>{failure.message}</span>
      <FailureDetail failure={failure} />
    </div>
  );
}

function LastCheckLine({ last }: { last: LastCheck }) {
  return (
    <div data-testid="updates-last-check" style={MUTED}>
      {last.at === 0 || last.state === ''
        ? describeLastCheck(last)
        : `Last checked ${relativeTime(last.at)} — ${describeLastCheck(last)}`}
    </div>
  );
}

/**
 * Settings -> Updates (P11.1). Every state the flow can be in is rendered here
 * and nowhere else: no toast, no modal, and nothing that acts on the user's
 * behalf. The buttons that write are only present while they are legal, so
 * "install" cannot be reached without asking for it first.
 */
export function UpdatesPanel() {
  const settings = useSettings((s) => s.settings);
  const set = useSettings((s) => s.set);
  const status = useUpdates((s) => s.status);
  const candidate = useUpdates((s) => s.candidate);
  const currentVersion = useUpdates((s) => s.currentVersion);
  const loadVersion = useUpdates((s) => s.loadVersion);
  const check = useUpdates((s) => s.check);
  const download = useUpdates((s) => s.download);
  const install = useUpdates((s) => s.install);
  const relaunch = useUpdates((s) => s.relaunch);

  React.useEffect(() => {
    void loadVersion();
  }, [loadVersion]);

  const last: LastCheck = {
    at: settings.updatesLastCheckAt,
    state: settings.updatesLastCheckState as LastCheck['state'],
    version: settings.updatesLastCheckVersion,
    error: settings.updatesLastCheckError,
  };
  const busy = status.kind === 'checking' || status.kind === 'downloading' || status.kind === 'installing';
  const percent = status.kind === 'downloading' ? downloadPercent(status.received, status.total) : null;
  const releaseDate = candidate ? formatReleaseDate(candidate.date) : null;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <span style={{ fontSize: 13 }} data-testid="updates-version">
          {currentVersion ? `Sift ${currentVersion}` : 'Sift'}
        </span>
        <LastCheckLine last={last} />
      </div>

      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
        <span style={{ fontSize: 13 }}>Check for updates automatically</span>
        <Switch
          checked={settings.updatesAutoCheck}
          ariaLabel="Check for updates automatically"
          onChange={(v) => void set({ updatesAutoCheck: v })}
        />
      </div>
      <div style={MUTED}>
        One check per launch. The check only reads the release manifest; Sift never downloads or installs
        anything unless you ask it to, and it skips the check while you are offline.
      </div>

      <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
        <Button data-testid="updates-check" disabled={busy} onClick={() => void check()}>
          {status.kind === 'checking' ? 'Checking…' : 'Check for updates'}
        </Button>
      </div>

      <div
        data-testid="updates-status"
        data-state={status.kind}
        role="status"
        aria-live="polite"
        style={{ display: 'flex', flexDirection: 'column', gap: 10 }}
      >
        {status.kind === 'checking' && <span style={{ fontSize: 13 }}>Checking for updates…</span>}

        {(status.kind === 'idle' || status.kind === 'up-to-date') && (
          <span style={{ fontSize: 13 }}>
            {status.kind === 'idle' ? 'Not checked in this session.' : "You're up to date."}
          </span>
        )}

        {status.kind === 'no-release' && (
          <span style={{ fontSize: 13 }} data-testid="updates-no-release">
            No published release yet. The update endpoint has no release manifest, so there is nothing to
            install — this is not a failure.
          </span>
        )}

        {status.kind === 'failed' && <FailureBlock failure={status.failure} />}

        {(status.kind === 'available' || status.kind === 'downloading' || status.kind === 'ready') &&
          candidate && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
              <span style={{ fontSize: 13 }} data-testid="updates-available">
                Sift {candidate.version} is available
                {releaseDate ? ` — released ${releaseDate}` : ''}.
              </span>
              {candidate.notes && (
                <div
                  data-testid="updates-notes"
                  style={{
                    ...MUTED,
                    whiteSpace: 'pre-wrap',
                    maxHeight: 160,
                    overflowY: 'auto',
                    border: '1px solid var(--border)',
                    borderRadius: 8,
                    padding: '8px 10px',
                    background: 'var(--n1)',
                  }}
                >
                  {candidate.notes}
                </div>
              )}

              {status.kind === 'available' && (
                <Button
                  data-testid="updates-download"
                  style={{ alignSelf: 'flex-start' }}
                  onClick={() => void download()}
                >
                  Download update
                </Button>
              )}

              {status.kind === 'downloading' && (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                  <progress
                    value={percent ?? undefined}
                    max={percent === null ? undefined : 100}
                    style={{ width: '100%' }}
                  />
                  <span style={MUTED} data-testid="updates-progress">
                    {percent === null ? 'Downloading…' : `Downloading… ${percent}%`}
                  </span>
                </div>
              )}

              {status.kind === 'ready' && (
                <>
                  <span style={{ fontSize: 13 }} data-testid="updates-ready">
                    Downloaded. The signature was verified by Sift's update key.
                  </span>
                  <Button
                    data-testid="updates-install"
                    style={{ alignSelf: 'flex-start' }}
                    onClick={() => void install()}
                  >
                    Install and quit to finish
                  </Button>
                </>
              )}
            </div>
          )}

        {status.kind === 'installing' && <span style={{ fontSize: 13 }}>Installing…</span>}

        {status.kind === 'installed' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <span style={{ fontSize: 13 }} data-testid="updates-installed">
              Installed. The new version runs after Sift restarts.
            </span>
            <Button
              data-testid="updates-relaunch"
              style={{ alignSelf: 'flex-start' }}
              onClick={() => void relaunch()}
            >
              Restart Sift now
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
