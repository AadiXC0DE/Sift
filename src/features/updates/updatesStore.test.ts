/**
 * The update flow's rules (P11.1): what may run by itself, what may only run
 * when the user asks, and what the app records afterwards.
 *
 * The transport is mocked at its factory, so everything under test here is the
 * real store: the once-per-launch latch, the offline short-circuit, the
 * explicit-action gates and the recorded observation.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { defaultSettings } from '../../app/ipc/types';
import { useSettings } from '../../stores/settingsStore';
import type { UpdateTransport } from './transport';
import { resetLaunchCheckLatch, useUpdates } from './updatesStore';

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

/**
 * Stage the transport the store will build. Each call to the factory returns a
 * fresh, numbered instance, so "the handle from the last check was closed
 * before the next one started" is an ordering the test can actually read.
 */
function stageTransport(overrides: Partial<UpdateTransport> = {}): string[] {
  const calls: string[] = [];
  let instance = 0;
  hoisted.make = () => {
    instance += 1;
    const tag = `#${instance}`;
    return {
      check: async () => {
        calls.push(`check${tag}`);
        return (overrides.check ?? (async () => null))();
      },
      download: async (onProgress) => {
        calls.push(`download${tag}`);
        return (overrides.download ?? (async () => {}))(onProgress);
      },
      install: async () => {
        calls.push(`install${tag}`);
        return (overrides.install ?? (async () => {}))();
      },
      relaunch: async () => {
        calls.push(`relaunch${tag}`);
        return (overrides.relaunch ?? (async () => {}))();
      },
      dispose: async () => {
        calls.push(`dispose${tag}`);
        return (overrides.dispose ?? (async () => {}))();
      },
    };
  };
  return calls;
}

const candidate = { version: '1.4.0', date: '2026-09-13T00:00:00Z' };

function settingsState(): {
  updatesLastCheckAt: number;
  updatesLastCheckState: string;
  updatesLastCheckVersion: string;
  updatesLastCheckError: string;
} {
  const s = useSettings.getState().settings;
  return {
    updatesLastCheckAt: s.updatesLastCheckAt,
    updatesLastCheckState: s.updatesLastCheckState,
    updatesLastCheckVersion: s.updatesLastCheckVersion,
    updatesLastCheckError: s.updatesLastCheckError,
  };
}

beforeEach(() => {
  hoisted.make = null;
  resetLaunchCheckLatch();
  useUpdates.setState({ status: { kind: 'idle' }, candidate: null, currentVersion: '' });
  useSettings.setState({ settings: { ...defaultSettings }, loaded: true });
  vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(true);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('automatic checks', () => {
  it('runs at most once per launch and records what it found', async () => {
    const calls = stageTransport();
    await useUpdates.getState().check({ automatic: true });
    await useUpdates.getState().check({ automatic: true });

    expect(calls.filter((c) => c.startsWith('check'))).toEqual(['check#1']);
    expect(useUpdates.getState().status.kind).toBe('up-to-date');
    expect(settingsState().updatesLastCheckState).toBe('up-to-date');
    expect(settingsState().updatesLastCheckAt).toBeGreaterThan(0);
  });

  it('is skipped entirely when the user turned it off', async () => {
    const calls = stageTransport();
    useSettings.setState({
      settings: { ...defaultSettings, updatesAutoCheck: false },
      loaded: true,
    });

    await useUpdates.getState().check({ automatic: true });

    expect(calls).toEqual([]);
    // Nothing was checked, so nothing is claimed.
    expect(settingsState().updatesLastCheckState).toBe('');
    expect(settingsState().updatesLastCheckAt).toBe(0);
  });

  it('still lets a manual check run with automatic checks turned off', async () => {
    const calls = stageTransport();
    useSettings.setState({
      settings: { ...defaultSettings, updatesAutoCheck: false },
      loaded: true,
    });

    await useUpdates.getState().check();

    expect(calls).toEqual(['check#1']);
  });

  it('sends nothing while the host is offline and says so', async () => {
    const calls = stageTransport();
    vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false);

    await useUpdates.getState().check({ automatic: true });

    expect(calls).toEqual([]);
    const status = useUpdates.getState().status;
    expect(status.kind).toBe('failed');
    if (status.kind !== 'failed') throw new Error('unreachable');
    expect(status.failure.code).toBe('offline');
    expect(settingsState().updatesLastCheckState).toBe('failed');
    expect(settingsState().updatesLastCheckError).toMatch(/offline/);
  });
});

describe('check outcomes', () => {
  it('records the version when an update is available', async () => {
    stageTransport({ check: async () => candidate });

    await useUpdates.getState().check();

    expect(useUpdates.getState().status.kind).toBe('available');
    expect(useUpdates.getState().candidate).toEqual(candidate);
    expect(settingsState().updatesLastCheckState).toBe('available');
    expect(settingsState().updatesLastCheckVersion).toBe('1.4.0');
  });

  it('reports a missing release as "no published release yet", not as a failure', async () => {
    stageTransport({
      check: async () => {
        throw new Error('Could not fetch a valid release JSON from the remote');
      },
    });

    await useUpdates.getState().check();

    expect(useUpdates.getState().status.kind).toBe('no-release');
    expect(settingsState().updatesLastCheckState).toBe('no-release');
    expect(settingsState().updatesLastCheckError).toBe('');
  });

  it('records the exact reason a check failed', async () => {
    stageTransport({
      check: async () => {
        throw new Error(
          'updater.check not allowed. Permissions associated with this command: updater:allow-check',
        );
      },
    });

    await useUpdates.getState().check();

    const status = useUpdates.getState().status;
    expect(status.kind).toBe('failed');
    if (status.kind !== 'failed') throw new Error('unreachable');
    expect(status.failure.code).toBe('permission');
    expect(settingsState().updatesLastCheckError).toBe(status.failure.message);
    expect(status.failure.detail).toMatch(/updater\.check not allowed/);
  });

  it('reports malformed metadata instead of pretending there is no update', async () => {
    stageTransport({
      check: async () => {
        throw new Error('missing field `platforms` at line 1 column 20');
      },
    });

    await useUpdates.getState().check();

    const status = useUpdates.getState().status;
    expect(status.kind).toBe('failed');
    if (status.kind !== 'failed') throw new Error('unreachable');
    expect(status.failure.code).toBe('metadata');
  });
});

describe('explicit actions', () => {
  it('never installs without a download the user asked for', async () => {
    const calls = stageTransport({ check: async () => candidate });
    await useUpdates.getState().check();

    await useUpdates.getState().install();

    expect(calls).not.toContain('install#1');
    expect(useUpdates.getState().status.kind).toBe('available');
  });

  it('never installs from a check alone, even after a fresh check', async () => {
    const calls = stageTransport({ check: async () => candidate });
    await useUpdates.getState().check();
    await useUpdates.getState().check({ automatic: true });

    expect(calls.filter((c) => c.startsWith('install'))).toEqual([]);
    expect(calls.filter((c) => c.startsWith('download'))).toEqual([]);
  });

  it('never downloads or relaunches on its own', async () => {
    const calls = stageTransport({ check: async () => candidate });
    await useUpdates.getState().check({ automatic: true });

    expect(calls.filter((c) => c.startsWith('download'))).toEqual([]);
    expect(calls.filter((c) => c.startsWith('install'))).toEqual([]);
    expect(calls.filter((c) => c.startsWith('relaunch'))).toEqual([]);
  });

  it('downloads only when asked, then waits for a separate install', async () => {
    const calls = stageTransport({ check: async () => candidate });
    await useUpdates.getState().check();

    await useUpdates.getState().download();

    expect(calls).toContain('download#1');
    expect(calls.filter((c) => c.startsWith('install'))).toEqual([]);
    expect(useUpdates.getState().status.kind).toBe('ready');

    await useUpdates.getState().install();

    expect(calls).toContain('install#1');
    expect(useUpdates.getState().status.kind).toBe('installed');
    expect(calls.filter((c) => c.startsWith('relaunch'))).toEqual([]);

    await useUpdates.getState().relaunch();

    expect(calls).toContain('relaunch#1');
  });

  it('reports download progress while the download runs', async () => {
    stageTransport({
      check: async () => candidate,
      download: async (onProgress) => {
        onProgress(0, 200);
        onProgress(50, 200);
        onProgress(200, 200);
      },
    });
    await useUpdates.getState().check();
    const progress: { received: number; total: number | null }[] = [];
    const unsubscribe = useUpdates.subscribe((state) => {
      if (state.status.kind === 'downloading') {
        progress.push({ received: state.status.received, total: state.status.total });
      }
    });

    await useUpdates.getState().download();
    unsubscribe();

    // The first entry is the transition into `downloading`, before the
    // transport has any header to report a total from.
    expect(progress).toEqual([
      { received: 0, total: null },
      { received: 0, total: 200 },
      { received: 50, total: 200 },
      { received: 200, total: 200 },
    ]);
    expect(useUpdates.getState().status.kind).toBe('ready');
  });

  it('refuses to install when the download failed verification', async () => {
    const calls = stageTransport({
      check: async () => candidate,
      download: async () => {
        throw new Error('Invalid encoding in minisign data');
      },
    });
    await useUpdates.getState().check();

    await useUpdates.getState().download();

    const status = useUpdates.getState().status;
    expect(status.kind).toBe('failed');
    if (status.kind !== 'failed') throw new Error('unreachable');
    expect(status.stage).toBe('download');
    expect(status.failure.code).toBe('signature');
    expect(status.failure.message).toMatch(/could not be verified/);

    await useUpdates.getState().install();

    expect(calls).not.toContain('install#1');
  });

  it('refuses to relaunch before the install succeeded', async () => {
    const calls = stageTransport({ check: async () => candidate });
    await useUpdates.getState().check();

    await useUpdates.getState().relaunch();

    expect(calls).not.toContain('relaunch#1');
  });

  it('reports an install failure without pretending it worked', async () => {
    stageTransport({
      check: async () => candidate,
      install: async () => {
        throw new Error('Failed to move the new app into place');
      },
    });
    await useUpdates.getState().check();
    await useUpdates.getState().download();

    await useUpdates.getState().install();

    const status = useUpdates.getState().status;
    expect(status.kind).toBe('failed');
    if (status.kind !== 'failed') throw new Error('unreachable');
    expect(status.stage).toBe('install');
    expect(status.failure.detail).toMatch(/Failed to move the new app into place/);
  });
});

describe('transport lifetime', () => {
  it('never runs two checks at once', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let started: () => void = () => {};
    const inFlight = new Promise<void>((resolve) => {
      started = resolve;
    });
    const calls = stageTransport({
      check: async () => {
        started();
        await gate;
        return null;
      },
    });

    const first = useUpdates.getState().check();
    await inFlight;
    // The second call arrives while the first is still waiting on the endpoint.
    await useUpdates.getState().check();

    expect(calls).toEqual(['check#1']);

    release();
    await first;

    expect(calls).toEqual(['check#1']);
    expect(useUpdates.getState().status.kind).toBe('up-to-date');
  });

  it('closes the plugin handle a failed check left behind', async () => {
    const calls = stageTransport({
      check: async () => {
        throw new Error('The signature verification failed');
      },
    });

    await useUpdates.getState().check();

    expect(calls).toEqual(['check#1', 'dispose#1']);
  });

  it('closes the handle before the next check replaces it', async () => {
    const calls = stageTransport();
    await useUpdates.getState().check();
    await useUpdates.getState().check();

    expect(calls).toEqual(['check#1', 'dispose#1', 'check#2']);
  });
});

describe('installed version', () => {
  it('comes from the app, not from a check', async () => {
    await useUpdates.getState().loadVersion();

    expect(useUpdates.getState().currentVersion).toBe('1.0.0');
  });
});
