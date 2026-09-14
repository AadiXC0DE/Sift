/**
 * Test-only replacement for `@tauri-apps/plugin-updater`.
 *
 * `e2e/vite.e2e.config.ts` aliases the real package here, so the production
 * bundle is untouched and the real `src/features/updates/transport.ts` is what
 * the browser suite drives. The stub answers only from the scenario the test
 * staged in `window.__SIFT_UPDATER__` before the page loaded, and records every
 * download and install it was asked for — so "the app refused to install" is an
 * observation, not an assertion about the mock's silence.
 *
 * The messages it throws are the plugin's own, quoted from
 * `tauri-plugin-updater-2.11.0/src/error.rs` and `minisign-verify-0.2.5`, so the
 * app's classification is exercised against the real wording. This module touches
 * `window` at import time: only the browser loads it, never the Playwright runner.
 */
import type { DownloadEvent } from '@tauri-apps/plugin-updater';
import { PLACEHOLDER_KEY_MESSAGE, type UpdaterControl, type UpdaterScenario } from './updater-scenario';

const DEFAULT_SCENARIO: UpdaterScenario = { kind: 'none' };

const state = {
  checks: 0,
  downloads: [] as string[],
  installs: [] as string[],
};

function scenario(): UpdaterScenario {
  return (window as unknown as { __SIFT_UPDATER__?: UpdaterScenario }).__SIFT_UPDATER__ ?? DEFAULT_SCENARIO;
}

/** Mirrors the `Update` resource: metadata plus the two actions that write. */
export class Update {
  readonly version: string;
  readonly date?: string;
  readonly body?: string;
  readonly currentVersion = '1.0.0';
  readonly rawJson: Record<string, unknown> = {};

  constructor(input: { version: string; notes?: string; date?: string }) {
    this.version = input.version;
    this.body = input.notes;
    this.date = input.date;
  }

  async download(onEvent?: (event: DownloadEvent) => void): Promise<void> {
    state.downloads.push(this.version);
    const current = scenario();
    if (current.kind === 'unverified') {
      throw new Error(current.message ?? PLACEHOLDER_KEY_MESSAGE);
    }
    onEvent?.({ event: 'Started', data: { contentLength: 4_000_000 } });
    onEvent?.({ event: 'Progress', data: { chunkLength: 3_000_000 } });
    onEvent?.({ event: 'Progress', data: { chunkLength: 1_000_000 } });
    onEvent?.({ event: 'Finished' });
  }

  async install(): Promise<void> {
    state.installs.push(this.version);
  }

  async close(): Promise<void> {}
}

export async function check(): Promise<Update | null> {
  state.checks += 1;
  const current = scenario();
  if (current.kind === 'reject') throw new Error(current.message);
  if (current.kind === 'none') return null;
  return new Update({ version: current.version, notes: current.notes, date: current.date });
}

function installControl(): void {
  const w = window as unknown as { __siftUpdater?: UpdaterControl };
  if (w.__siftUpdater) return;
  w.__siftUpdater = {
    scenario,
    checks: () => state.checks,
    downloads: () => [...state.downloads],
    installs: () => [...state.installs],
    resets: () => {
      state.checks = 0;
      state.downloads = [];
      state.installs = [];
    },
  };
}

installControl();
