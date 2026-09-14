import type { Page } from '@playwright/test';
import {
  NO_RELEASE_MESSAGE,
  PLACEHOLDER_KEY_MESSAGE,
  type UpdaterScenario,
} from './fixture/updater-scenario';
import { expect, test } from './fixture/test';

/**
 * Settings -> Updates, driven through the real flow in a browser (P11.1).
 *
 * The updater stub is aliased in for `@tauri-apps/plugin-updater`
 * (`e2e/vite.e2e.config.ts`), so this exercises the app's own transport,
 * store and panel. The scenarios are staged before the page loads, which is
 * what the launch check sees.
 */
async function stage(page: Page, scenario: UpdaterScenario): Promise<void> {
  await page.addInitScript((value) => {
    (window as unknown as { __SIFT_UPDATER__?: unknown }).__SIFT_UPDATER__ = value;
  }, scenario);
}

async function openUpdates(page: Page) {
  await page.keyboard.press('Meta+,');
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Updates', exact: true }).click();
  return dialog;
}

test('updates: the launch check runs once and reports being up to date', async ({ page, app }) => {
  await stage(page, { kind: 'none' });
  await app.gotoApp();
  await app.ready();
  const dialog = await openUpdates(page);

  await expect(dialog.getByTestId('updates-version')).toHaveText('Sift e2e-fixture');
  await expect(dialog.getByTestId('updates-status')).toHaveAttribute('data-state', 'up-to-date');
  await expect(dialog.getByTestId('updates-status')).toContainText("You're up to date.");

  const asked = await page.evaluate(() => ({
    checks: window.__siftUpdater?.checks() ?? -1,
    downloads: window.__siftUpdater?.downloads() ?? ['missing'],
    installs: window.__siftUpdater?.installs() ?? ['missing'],
  }));
  expect(asked.checks).toBe(1);
  // A check is a read: it never downloads and never installs.
  expect(asked.downloads).toEqual([]);
  expect(asked.installs).toEqual([]);
});

test('updates: an endpoint with nothing published reads as no release yet', async ({ page, app }) => {
  await stage(page, { kind: 'reject', message: NO_RELEASE_MESSAGE });
  await app.gotoApp();
  await app.ready();
  const dialog = await openUpdates(page);

  await expect(dialog.getByTestId('updates-status')).toHaveAttribute('data-state', 'no-release');
  await expect(dialog.getByTestId('updates-no-release')).toContainText('No published release yet.');
  // Not a failure the user has to act on, and nothing to install.
  await expect(dialog.getByTestId('updates-failure')).toHaveCount(0);
  await expect(dialog.getByTestId('updates-download')).toHaveCount(0);
  expect(await page.evaluate(() => window.__siftUpdater?.installs() ?? ['missing'])).toEqual([]);
});

test('updates: an update that fails signature verification is never installed', async ({ page, app }) => {
  await stage(page, { kind: 'unverified', version: '9.9.9', notes: 'Adds nothing real.' });
  await app.gotoApp();
  await app.ready();
  const dialog = await openUpdates(page);

  await expect(dialog.getByTestId('updates-available')).toContainText('Sift 9.9.9 is available');
  await expect(dialog.getByTestId('updates-notes')).toContainText('Adds nothing real.');
  await dialog.getByTestId('updates-download').click();

  const failure = dialog.getByTestId('updates-failure');
  await expect(failure).toHaveAttribute('data-code', 'signature');
  await expect(failure).toContainText('The update could not be verified, so it was not installed.');
  // The reason is the plugin's own words, not a truncated summary.
  await expect(dialog.getByTestId('updates-detail')).toContainText(PLACEHOLDER_KEY_MESSAGE);

  // No install control appeared, nothing was installed, and the app did not
  // fall over: the panel above is still rendered.
  await expect(dialog.getByTestId('updates-install')).toHaveCount(0);
  await expect(dialog.getByTestId('updates-ready')).toHaveCount(0);
  await expect(dialog.getByTestId('updates-version')).toBeVisible();
  const asked = await page.evaluate(() => ({
    downloads: window.__siftUpdater?.downloads() ?? ['missing'],
    installs: window.__siftUpdater?.installs() ?? ['missing'],
  }));
  expect(asked.downloads).toEqual(['9.9.9']);
  expect(asked.installs).toEqual([]);
});

test('updates: turning automatic checks off survives a restart of the app', async ({ page, app }) => {
  await stage(page, { kind: 'none' });
  await app.gotoApp();
  await app.ready();
  const dialog = await openUpdates(page);

  const toggle = dialog.getByRole('switch', { name: 'Check for updates automatically' });
  await expect(toggle).toBeChecked();
  await toggle.click();
  await expect(toggle).not.toBeChecked();

  await page.reload();
  await app.ready();
  const reopened = await openUpdates(page);
  await expect(reopened.getByRole('switch', { name: 'Check for updates automatically' })).not.toBeChecked();

  // A fresh page load is a fresh launch: with the switch off, nothing is asked.
  const checks = await page.evaluate(() => window.__siftUpdater?.checks() ?? -1);
  expect(checks).toBe(0);
});
