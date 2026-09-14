/**
 * The app's bridge to `tauri-plugin-updater` (P11.1).
 *
 * The plugin's JS wrapper is the only thing faked here, so what is asserted is
 * the mapping the app is responsible for: the bounded timeout it asks for, the
 * metadata it reads back, the progress it accumulates from the download
 * channel, and the fact that installing releases the Rust-side handle.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CheckOptions, DownloadEvent } from '@tauri-apps/plugin-updater';
import { api } from '../../app/ipc/commands';
import { createTransport, UPDATE_CHECK_TIMEOUT_MS } from './transport';

interface FakeUpdate {
  version: string;
  date?: string;
  body?: string;
  currentVersion: string;
  events: DownloadEvent[];
  failDownload: Error | null;
  installed: number;
  closed: number;
  download: (onEvent?: (event: DownloadEvent) => void) => Promise<void>;
  install: () => Promise<void>;
  close: () => Promise<void>;
}

const hoisted = vi.hoisted(() => ({ pluginCheck: vi.fn() }));

vi.mock('../../app/ipc/commands', () => ({
  api: { app_relaunch: vi.fn(async () => undefined) },
}));

vi.mock('@tauri-apps/plugin-updater', () => ({
  check: (options?: CheckOptions) => hoisted.pluginCheck(options),
  Update: class {},
}));

/** Stand in for the `Update` resource the plugin hands back. */
function stageUpdate(overrides: { events?: DownloadEvent[]; failDownload?: Error } = {}): FakeUpdate {
  const update: FakeUpdate = {
    version: '1.4.0',
    date: '2026-09-13T00:00:00Z',
    body: 'Fixes IMAP reconnect storms.',
    currentVersion: '1.0.0',
    events: overrides.events ?? [],
    failDownload: overrides.failDownload ?? null,
    installed: 0,
    closed: 0,
    async download(onEvent) {
      if (update.failDownload) throw update.failDownload;
      for (const event of update.events) onEvent?.(event);
    },
    async install() {
      update.installed += 1;
    },
    async close() {
      update.closed += 1;
    },
  };
  hoisted.pluginCheck.mockResolvedValue(update);
  return update;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('check', () => {
  it('bounds the request and reads the release metadata back', async () => {
    stageUpdate();

    const candidate = await createTransport().check();

    expect(hoisted.pluginCheck).toHaveBeenCalledWith({ timeout: UPDATE_CHECK_TIMEOUT_MS });
    expect(candidate).toEqual({
      version: '1.4.0',
      date: '2026-09-13T00:00:00Z',
      notes: 'Fixes IMAP reconnect storms.',
    });
  });

  it('reports "nothing newer" as null rather than as a candidate', async () => {
    hoisted.pluginCheck.mockResolvedValue(null);

    await expect(createTransport().check()).resolves.toBeNull();
  });

  it('lets the plugin error through untouched for the caller to classify', async () => {
    hoisted.pluginCheck.mockRejectedValue(new Error('Could not fetch a valid release JSON from the remote'));

    await expect(createTransport().check()).rejects.toThrow(
      'Could not fetch a valid release JSON from the remote',
    );
  });
});

describe('download', () => {
  it('turns the download channel into the progress the panel shows', async () => {
    stageUpdate({
      events: [
        { event: 'Started', data: { contentLength: 200 } },
        { event: 'Progress', data: { chunkLength: 50 } },
        { event: 'Progress', data: { chunkLength: 150 } },
        { event: 'Finished' },
      ],
    });
    const transport = createTransport();
    await transport.check();
    const progress: [number, number | null][] = [];

    await transport.download((received, total) => progress.push([received, total]));

    expect(progress).toEqual([
      [0, 200],
      [50, 200],
      [200, 200],
      [200, 200],
    ]);
  });

  it('reports an unknown total when the server sends no content length', async () => {
    stageUpdate({
      events: [
        { event: 'Started', data: {} },
        { event: 'Progress', data: { chunkLength: 7 } },
      ],
    });
    const transport = createTransport();
    await transport.check();
    const progress: [number, number | null][] = [];

    await transport.download((received, total) => progress.push([received, total]));

    expect(progress).toEqual([
      [0, null],
      [7, null],
      [7, null],
    ]);
  });

  it('refuses to download before a check found anything', async () => {
    await expect(createTransport().download(() => {})).rejects.toThrow(/before a check found an update/);
  });

  it('passes a verification failure straight through', async () => {
    stageUpdate({ failDownload: new Error('Invalid encoding in minisign data') });
    const transport = createTransport();
    await transport.check();

    await expect(transport.download(() => {})).rejects.toThrow('Invalid encoding in minisign data');
  });
});

describe('install and relaunch', () => {
  it('installs the downloaded update, then releases the handle', async () => {
    const update = stageUpdate();
    const transport = createTransport();
    await transport.check();
    await transport.download(() => {});

    await transport.install();

    expect(update.installed).toBe(1);
    expect(update.closed).toBe(1);
  });

  it('refuses to install before a check found anything', async () => {
    await expect(createTransport().install()).rejects.toThrow(/before a check found an update/);
  });

  it("restarts through the app's own command", async () => {
    await createTransport().relaunch();

    expect(vi.mocked(api.app_relaunch)).toHaveBeenCalledTimes(1);
  });
});

describe('dispose', () => {
  it('closes the handle once and stays safe to call again', async () => {
    const update = stageUpdate();
    const transport = createTransport();
    await transport.check();

    await transport.dispose();
    await transport.dispose();

    expect(update.closed).toBe(1);
  });
});
