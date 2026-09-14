/**
 * Settings -> Updates (P11.1), rendered for every state the flow can be in.
 *
 * The panel is the only surface that reports updates, so what it says — and
 * what it refuses to offer — is the whole contract: an unverified download
 * reads as a message, not a crash; "no published release yet" reads as a fact,
 * not an error; and no install control exists until the user has asked for a
 * download and it has finished.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { defaultSettings } from '../../app/ipc/types';
import { useSettings } from '../../stores/settingsStore';
import { api } from '../../app/ipc/commands';
import type { UpdateTransport } from './transport';
import { resetLaunchCheckLatch, useUpdates } from './updatesStore';
import { UpdatesPanel } from './UpdatesPanel';

const hoisted = vi.hoisted(() => ({ make: null as null | (() => UpdateTransport) }));

vi.mock('../../app/ipc/commands', () => ({
  api: {
    system_info: vi.fn(async () => ({ version: '1.0.0', oauth_available: false, demo: false })),
    settings_set: vi.fn(async (patch: Record<string, unknown>) => patch),
    app_relaunch: vi.fn(async () => undefined),
  },
}));

vi.mock('./transport', () => ({
  UPDATE_CHECK_TIMEOUT_MS: 15_000,
  createTransport: () => {
    if (!hoisted.make) throw new Error('no transport was staged for this test');
    return hoisted.make();
  },
}));

/** Records what the app asked the plugin to do, in order. */
function stageTransport(overrides: Partial<UpdateTransport> = {}): string[] {
  const calls: string[] = [];
  hoisted.make = () => ({
    check: async () => {
      calls.push('check');
      return (overrides.check ?? (async () => null))();
    },
    download: async (onProgress) => {
      calls.push('download');
      return (overrides.download ?? (async () => {}))(onProgress);
    },
    install: async () => {
      calls.push('install');
      return (overrides.install ?? (async () => {}))();
    },
    relaunch: async () => {
      calls.push('relaunch');
      return (overrides.relaunch ?? (async () => {}))();
    },
    dispose: async () => {
      calls.push('dispose');
      return (overrides.dispose ?? (async () => {}))();
    },
  });
  return calls;
}

const candidate = {
  version: '1.4.0',
  currentVersion: '1.0.0',
  date: '2026-09-13T00:00:00Z',
  notes: 'Fixes IMAP reconnect storms.',
};

beforeEach(() => {
  hoisted.make = null;
  resetLaunchCheckLatch();
  useUpdates.setState({ status: { kind: 'idle' }, candidate: null, currentVersion: '1.0.0' });
  useSettings.setState({ settings: { ...defaultSettings }, loaded: true });
  vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(true);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Settings -> Updates', () => {
  it('names the installed version and says when the last check happened', () => {
    useSettings.setState({
      settings: {
        ...defaultSettings,
        updatesLastCheckAt: Date.now() - 90_000,
        updatesLastCheckState: 'up-to-date',
      },
      loaded: true,
    });

    render(<UpdatesPanel />);

    expect(screen.getByTestId('updates-version')).toHaveTextContent('Sift 1.0.0');
    expect(screen.getByTestId('updates-last-check')).toHaveTextContent(
      /Last checked \d+ min ago — Up to date\./,
    );
  });

  it('never claims a check that has not happened', () => {
    render(<UpdatesPanel />);

    expect(screen.getByTestId('updates-last-check')).toHaveTextContent('Never checked for updates.');
    expect(screen.getByTestId('updates-status')).toHaveAttribute('data-state', 'idle');
    expect(screen.getByTestId('updates-status')).toHaveTextContent('Not checked in this session.');
  });

  it('reports an available update with its version, date and notes', () => {
    useUpdates.setState({ status: { kind: 'available' }, candidate });

    render(<UpdatesPanel />);

    const available = screen.getByTestId('updates-available');
    expect(available).toHaveTextContent('Sift 1.4.0 is available');
    expect(available.textContent).toMatch(/released .+\d/);
    expect(screen.getByTestId('updates-notes')).toHaveTextContent('Fixes IMAP reconnect storms.');
    expect(screen.getByTestId('updates-download')).toBeInTheDocument();
    // Nothing is installed and nothing may be installed from here.
    expect(screen.queryByTestId('updates-install')).not.toBeInTheDocument();
    expect(screen.queryByTestId('updates-relaunch')).not.toBeInTheDocument();
  });

  it('shows visible progress while the download runs', () => {
    useUpdates.setState({
      status: { kind: 'downloading', received: 3_000_000, total: 12_000_000 },
      candidate,
    });

    render(<UpdatesPanel />);

    expect(screen.getByTestId('updates-progress')).toHaveTextContent('Downloading… 25%');
    expect(screen.queryByTestId('updates-install')).not.toBeInTheDocument();
    expect(screen.queryByTestId('updates-download')).not.toBeInTheDocument();
  });

  it('offers install only after a finished, verified download', async () => {
    const calls = stageTransport({ check: async () => candidate });
    await useUpdates.getState().check();
    const user = userEvent.setup();

    render(<UpdatesPanel />);
    await user.click(screen.getByTestId('updates-download'));

    expect(screen.getByTestId('updates-ready')).toHaveTextContent(/signature was verified/);
    expect(calls).not.toContain('install');

    await user.click(screen.getByTestId('updates-install'));

    expect(calls).toContain('install');
    expect(screen.getByTestId('updates-installed')).toHaveTextContent(/runs after Sift restarts/);

    await user.click(screen.getByTestId('updates-relaunch'));

    expect(calls).toContain('relaunch');
  });

  it('reports an unverifiable download as a message and never offers install', async () => {
    const calls = stageTransport({
      check: async () => candidate,
      download: async () => {
        // Exactly what the placeholder pubkey produces in `verify_signature`.
        throw new Error('Invalid encoding in minisign data');
      },
    });
    await useUpdates.getState().check();
    const user = userEvent.setup();

    render(<UpdatesPanel />);
    await user.click(screen.getByTestId('updates-download'));

    const failure = screen.getByTestId('updates-failure');
    expect(failure).toHaveAttribute('data-code', 'signature');
    expect(failure).toHaveTextContent('The update could not be verified, so it was not installed.');
    expect(screen.getByTestId('updates-detail')).toHaveTextContent('Invalid encoding in minisign data');
    // No crash, no install control, and nothing was installed.
    expect(screen.queryByTestId('updates-install')).not.toBeInTheDocument();
    expect(screen.queryByTestId('updates-ready')).not.toBeInTheDocument();
    expect(calls).not.toContain('install');
  });

  it('reports an offline check without any network call', async () => {
    const calls = stageTransport();
    vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false);
    const user = userEvent.setup();

    render(<UpdatesPanel />);
    await user.click(screen.getByTestId('updates-check'));

    expect(calls).toEqual([]);
    const failure = screen.getByTestId('updates-failure');
    expect(failure).toHaveAttribute('data-code', 'offline');
    expect(failure).toHaveTextContent('Could not check for updates: this Mac is offline.');
  });

  it('reports a missing release as a fact, not as a failure', async () => {
    stageTransport({
      check: async () => {
        throw new Error('Could not fetch a valid release JSON from the remote');
      },
    });
    const user = userEvent.setup();

    render(<UpdatesPanel />);
    await user.click(screen.getByTestId('updates-check'));

    expect(screen.getByTestId('updates-status')).toHaveAttribute('data-state', 'no-release');
    expect(screen.getByTestId('updates-no-release')).toHaveTextContent(
      'No published release yet. The update endpoint has no release manifest, so there is nothing to install — this is not a failure.',
    );
    expect(screen.queryByTestId('updates-failure')).not.toBeInTheDocument();
    expect(screen.queryByTestId('updates-download')).not.toBeInTheDocument();
  });

  it('reports a truncated or unreadable manifest as a failure, quoting the plugin', async () => {
    stageTransport({
      check: async () => {
        throw new Error('missing field `platforms` at line 1 column 20');
      },
    });
    const user = userEvent.setup();

    render(<UpdatesPanel />);
    await user.click(screen.getByTestId('updates-check'));

    const failure = screen.getByTestId('updates-failure');
    expect(failure).toHaveAttribute('data-code', 'metadata');
    expect(failure).toHaveTextContent('cannot read');
    expect(screen.getByTestId('updates-detail')).toHaveTextContent(
      'missing field `platforms` at line 1 column 20',
    );
  });

  it('says it is up to date when the endpoint publishes nothing newer', async () => {
    stageTransport({ check: async () => null });
    const user = userEvent.setup();

    render(<UpdatesPanel />);
    await user.click(screen.getByTestId('updates-check'));

    expect(screen.getByTestId('updates-status')).toHaveAttribute('data-state', 'up-to-date');
    expect(screen.getByTestId('updates-status')).toHaveTextContent("You're up to date.");
    expect(screen.queryByTestId('updates-download')).not.toBeInTheDocument();
    expect(screen.queryByTestId('updates-failure')).not.toBeInTheDocument();
  });

  it('turns the launch check back on and persists the choice', async () => {
    useSettings.setState({
      settings: { ...defaultSettings, updatesAutoCheck: false },
      loaded: true,
    });
    const user = userEvent.setup();

    render(<UpdatesPanel />);
    await user.click(screen.getByRole('switch', { name: 'Check for updates automatically' }));

    expect(vi.mocked(api.settings_set)).toHaveBeenCalledWith({ updatesAutoCheck: true });
    expect(useSettings.getState().settings.updatesAutoCheck).toBe(true);
  });

  it('ships the automatic check switched on', () => {
    render(<UpdatesPanel />);

    expect(screen.getByRole('switch', { name: 'Check for updates automatically' })).toBeChecked();
  });
});
