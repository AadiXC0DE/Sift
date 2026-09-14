/**
 * The updater scenario vocabulary, shared by the browser stub and the spec that
 * stages it.
 *
 * This module is deliberately free of browser globals: the Playwright runner
 * imports it in Node, while `tauri-updater.ts` (which installs itself on
 * `window`) is only ever loaded inside the page.
 */

/** The release metadata a scenario announces. */
export type FixtureRelease = { version: string; notes?: string; date?: string };

export type UpdaterScenario =
  /** The endpoint publishes nothing newer: `check()` resolves to null. */
  | { kind: 'none' }
  /** The check itself fails, with the plugin's message. */
  | { kind: 'reject'; message: string }
  | ({ kind: 'available' } & FixtureRelease)
  /** A release exists but the artifact fails verification on download. */
  | ({ kind: 'unverified'; message?: string } & FixtureRelease);

/** The plugin's wording for a release manifest that no endpoint produced. */
export const NO_RELEASE_MESSAGE = 'Could not fetch a valid release JSON from the remote';

/** What the placeholder pubkey in `tauri.conf.json` produces today. */
export const PLACEHOLDER_KEY_MESSAGE = 'Invalid encoding in minisign data';

export interface UpdaterControl {
  scenario: () => UpdaterScenario;
  checks: () => number;
  downloads: () => string[];
  installs: () => string[];
  resets: () => void;
}
