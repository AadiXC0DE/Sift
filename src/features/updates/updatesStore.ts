/**
 * The update flow (P11.1).
 *
 * Three rules shape everything here, and each has a test:
 *
 * 1. Checking is a read. It may run by itself once per launch, and it is
 *    skipped when the host is offline or the user turned it off.
 * 2. Downloading and installing are writes the user asks for. `download` is
 *    reachable only from `available`, `install` only from `ready`, `relaunch`
 *    only from `installed` — every other call is a no-op that touches nothing.
 * 3. A failure is never dressed up as success. An endpoint with no published
 *    release is reported as "no release yet", not as an error; a failed check
 *    is reported with the exact reason the plugin gave.
 */
import { create } from 'zustand';
import { api } from '../../app/ipc/commands';
import type { Settings } from '../../app/ipc/types';
import { useSettings } from '../../stores/settingsStore';
import { createTransport, type UpdateTransport } from './transport';
import {
  checkOutcomeFromError,
  classifyUpdateError,
  lastCheckStateOf,
  type CheckOutcome,
  type UpdateCandidate,
  type UpdateFailure,
  type UpdateStage,
} from './updates';

export type UpdateStatus =
  /** Nothing has been checked in this window yet. */
  | { kind: 'idle' }
  | { kind: 'checking' }
  /** The endpoint answered and published nothing newer. */
  | { kind: 'up-to-date' }
  /** The endpoint answered with no release manifest at all; not an error. */
  | { kind: 'no-release' }
  | { kind: 'available' }
  | { kind: 'downloading'; received: number; total: number | null }
  /** Downloaded and signature-verified; waiting for the user to install it. */
  | { kind: 'ready' }
  | { kind: 'installing' }
  /** Installed; the new build runs after a restart. */
  | { kind: 'installed' }
  | { kind: 'failed'; stage: UpdateStage; failure: UpdateFailure };

interface UpdatesStore {
  status: UpdateStatus;
  /** Present from `available` on; cleared when the flow restarts. */
  candidate: UpdateCandidate | null;
  /** The installed version, from the app itself rather than from a check. */
  currentVersion: string;

  loadVersion: () => Promise<void>;
  /**
   * `automatic` is the launch check: it honours the setting, runs at most once
   * per launch and is skipped offline. A manual check always runs and records
   * why if it cannot.
   */
  check: (options?: { automatic?: boolean }) => Promise<void>;
  download: () => Promise<void>;
  install: () => Promise<void>;
  relaunch: () => Promise<void>;
}

/** One launch means one automatic check, however many components mount. */
let autoCheckedThisLaunch = false;

/** Reset the once-per-launch latch. Test seam only. */
export function resetLaunchCheckLatch(): void {
  autoCheckedThisLaunch = false;
}

export const useUpdates = create<UpdatesStore>((set, get) => {
  /** The transport bound to the current candidate; replaced by each check. */
  let transport: UpdateTransport | null = null;

  const record = (outcome: CheckOutcome) => {
    const patch: Record<string, string | number> = {
      updatesLastCheckAt: Date.now(),
      updatesLastCheckState: lastCheckStateOf(outcome),
      updatesLastCheckVersion: outcome.kind === 'available' ? outcome.candidate.version : '',
      updatesLastCheckError: outcome.kind === 'failed' ? outcome.failure.message : '',
    };
    // Persisted through settings so "last checked" survives a restart instead of
    // resetting on every launch. This writes the observation directly rather
    // than going through `useSettings.set`: a check result is not a preference,
    // so it must not re-apply the theme or the pane layout as a side effect.
    useSettings.setState({ settings: { ...useSettings.getState().settings, ...patch } });
    void api.settings_set(patch as Partial<Settings>).catch(() => {
      /* offline: the in-memory record still reports what happened */
    });
  };

  const fail = (stage: UpdateStage, error: unknown) => {
    const failure = classifyUpdateError(error, { stage, online: navigator.onLine !== false });
    set({ status: { kind: 'failed', stage, failure } });
  };

  return {
    status: { kind: 'idle' },
    candidate: null,
    currentVersion: '',

    loadVersion: async () => {
      if (get().currentVersion) return;
      try {
        const info = await api.system_info();
        set({ currentVersion: info.version });
      } catch {
        // The version row stays empty rather than claiming one.
      }
    },

    check: async (options = {}) => {
      const automatic = options.automatic === true;
      // One check at a time: a second click, or the launch check arriving while
      // the user is already checking, must not start a parallel run.
      const busy =
        get().status.kind === 'checking' ||
        get().status.kind === 'downloading' ||
        get().status.kind === 'installing';
      if (busy) return;
      if (automatic) {
        if (autoCheckedThisLaunch) return;
        autoCheckedThisLaunch = true;
        if (!useSettings.getState().settings.updatesAutoCheck) return;
      }
      // Offline is decided before any IPC call: nothing is sent, and the record
      // says exactly that instead of a fabricated result. Read once, because
      // the browser can change it mid-check and the outcome must describe the
      // attempt that was actually made.
      const online = navigator.onLine !== false;
      if (!online) {
        const outcome: CheckOutcome = {
          kind: 'failed',
          failure: classifyUpdateError('offline', { stage: 'check', online: false }),
        };
        set({ status: { kind: 'failed', stage: 'check', failure: outcome.failure }, candidate: null });
        record(outcome);
        return;
      }

      // Claim the flow before the first await: closing the previous handle is
      // asynchronous, and a second call (a double click, or the launch check
      // arriving mid-click) would otherwise see `idle` and start a parallel
      // check against the same endpoint.
      set({ status: { kind: 'checking' }, candidate: null });
      await transport?.dispose().catch(() => {});
      transport = createTransport();
      let outcome: CheckOutcome;
      try {
        const candidate = await transport.check();
        outcome = candidate ? { kind: 'available', candidate } : { kind: 'up-to-date' };
      } catch (error) {
        outcome = checkOutcomeFromError(error, { online });
        // A retired update handle is still a handle to close.
        await transport.dispose().catch(() => {});
      }
      if (outcome.kind === 'available') {
        set({ status: { kind: 'available' }, candidate: outcome.candidate });
      } else if (outcome.kind === 'up-to-date') {
        set({ status: { kind: 'up-to-date' }, candidate: null });
      } else if (outcome.kind === 'no-release') {
        set({ status: { kind: 'no-release' }, candidate: null });
      } else {
        set({ status: { kind: 'failed', stage: 'check', failure: outcome.failure }, candidate: null });
      }
      record(outcome);
    },

    download: async () => {
      // Only an update we found and the user asked for can be downloaded.
      if (get().status.kind !== 'available' || !transport) return;
      set({ status: { kind: 'downloading', received: 0, total: null } });
      try {
        await transport.download((received, total) =>
          set({ status: { kind: 'downloading', received, total } }),
        );
        set({ status: { kind: 'ready' } });
      } catch (error) {
        fail('download', error);
      }
    },

    install: async () => {
      if (get().status.kind !== 'ready' || !transport) return;
      set({ status: { kind: 'installing' } });
      try {
        await transport.install();
        set({ status: { kind: 'installed' } });
      } catch (error) {
        fail('install', error);
      }
    },

    relaunch: async () => {
      if (get().status.kind !== 'installed' || !transport) return;
      try {
        await transport.relaunch();
      } catch (error) {
        fail('relaunch', error);
      }
    },
  };
});
