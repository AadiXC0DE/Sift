/**
 * The only place in the app that talks to `tauri-plugin-updater` (P11.1).
 *
 * Everything the plugin does — fetching `latest.json`, picking the artifact for
 * this target, downloading it and verifying its minisign signature against
 * `plugins.updater.pubkey` — happens in Rust, behind these four calls. The
 * store above this file owns the user's choices; this file owns the bridge.
 * Keeping it in one module means a mock replaces the bridge instead of the
 * flow, so the tests exercise the real state machine.
 */
import { check as pluginCheck, type Update } from '@tauri-apps/plugin-updater';
import { api } from '../../app/ipc/commands';
import type { UpdateCandidate } from './updates';

/**
 * A check is a network read the user did not ask for when it runs at launch, so
 * it is bounded. The plugin applies this as the reqwest client timeout.
 */
export const UPDATE_CHECK_TIMEOUT_MS = 15_000;

/** Download progress as the plugin reports it; `total` is null until the headers arrive. */
export type DownloadProgress = (received: number, total: number | null) => void;

export interface UpdateTransport {
  /** `null` means the endpoint has no newer version. Throws on every failure. */
  check: () => Promise<UpdateCandidate | null>;
  download: (onProgress: DownloadProgress) => Promise<void>;
  install: () => Promise<void>;
  /** Restart into the installed build. Never returns when it succeeds. */
  relaunch: () => Promise<void>;
  /** Release the plugin handle; safe to call more than once. */
  dispose: () => Promise<void>;
}

/**
 * A transport bound to one check. The `Update` handle it holds is a Rust-side
 * resource, so it is closed as soon as the flow is done with it rather than
 * left for the window to unload.
 */
export function createTransport(): UpdateTransport {
  let handle: Update | null = null;
  let downloadedBytes = 0;

  const close = async () => {
    const held = handle;
    handle = null;
    if (held) await held.close().catch(() => {});
  };

  return {
    check: async () => {
      await close();
      downloadedBytes = 0;
      const update = await pluginCheck({ timeout: UPDATE_CHECK_TIMEOUT_MS });
      if (!update) return null;
      handle = update;
      return {
        version: update.version,
        date: update.date,
        notes: update.body,
      };
    },

    download: async (onProgress) => {
      if (!handle) throw new Error('Update.download called before a check found an update');
      let total: number | null = null;
      downloadedBytes = 0;
      await handle.download((event) => {
        if (event.event === 'Started') {
          total = event.data.contentLength ?? null;
          onProgress(0, total);
          return;
        }
        if (event.event === 'Progress') {
          downloadedBytes += event.data.chunkLength;
          onProgress(downloadedBytes, total);
        }
      });
      onProgress(downloadedBytes, total);
    },

    install: async () => {
      if (!handle) throw new Error('Update.install called before a check found an update');
      await handle.install();
      await close();
    },

    relaunch: async () => {
      // On macOS and Linux the plugin replaces the bundle in place and the
      // running process is still the old binary, so the restart is a separate
      // step the user takes after the install has succeeded. `plugin-process`
      // is not a dependency; the Rust command calls `AppHandle::request_restart`.
      await api.app_relaunch();
    },

    dispose: close,
  };
}
