import { expect, test } from './fixture/test';
import { fixtureCalls } from './fixture/helpers';

/**
 * P4.6: connectivity is per account, evidence-based and never blocks a cached
 * read. The frontend bridges the host's reachability hint and renders the
 * native state; recovery is a provider operation succeeding for that account.
 */
test('connectivity: an unreachable account is named inline and cached rows stay', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  const strip = page.getByTestId('connectivity-strip');
  await expect(strip).toHaveCount(0);

  await page.evaluate(() => {
    window.__siftFixture!.control.failAccount(
      window.__siftFixture!.ids.accountA,
      'connectivity',
      'IMAP connection failed',
    );
  });

  const rowA = strip.locator('[data-account-id="acc-a"]');
  await expect(rowA).toHaveAttribute('data-state', 'offline');
  await expect(rowA).toContainText('ada@example.test');
  await expect(rowA).toContainText('IMAP connection failed');
  // Truthful last-success timestamp, not a hardcoded "just now".
  await expect(rowA).toContainText(/Last synced .+/);

  // The cached window is untouched: an unreachable account hides no mail.
  expect((await app.rowIds()).length).toBeGreaterThan(0);
  await expect(app.row('blue-00')).toHaveCount(1);
});

test('connectivity: recovering one account clears only that account error', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await page.evaluate(() => {
    const control = window.__siftFixture!.control;
    control.failAccount(window.__siftFixture!.ids.accountA, 'connectivity', 'Offline');
    control.failAccount(window.__siftFixture!.ids.accountB, 'connectivity', 'Offline');
  });

  const strip = page.getByTestId('connectivity-strip');
  await expect(strip.locator('[data-account-id]')).toHaveCount(2);

  // Retry re-runs one sync tick for the account the reader asked about; the
  // other account keeps its own error.
  await strip
    .locator('[data-account-id="acc-a"]')
    .getByTestId('connectivity-retry')
    .click();

  await expect(strip.locator('[data-account-id="acc-a"]')).toHaveCount(0);
  await expect(strip.locator('[data-account-id="acc-b"]')).toHaveAttribute('data-state', 'offline');

  const calls = await fixtureCalls(page, 'sync_now');
  expect(calls.some((c) => c.accountId === 'acc-a')).toBe(true);
});

test('connectivity: the host offline/online events reach the backend hint', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await page.evaluate(() => {
    Object.defineProperty(navigator, 'onLine', { value: false, configurable: true });
    window.dispatchEvent(new Event('offline'));
  });

  const strip = page.getByTestId('connectivity-strip');
  await expect(strip.locator('[data-account-id="acc-a"]')).toHaveAttribute('data-state', 'offline');
  await expect(strip.locator('[data-account-id="acc-b"]')).toHaveAttribute('data-state', 'offline');

  await page.evaluate(() => {
    Object.defineProperty(navigator, 'onLine', { value: true, configurable: true });
    window.dispatchEvent(new Event('online'));
  });
  await expect(strip).toHaveCount(0);

  const hints = await fixtureCalls(page, 'app_network_hint');
  expect(hints.some((h) => h.online === false)).toBe(true);
  expect(hints.some((h) => h.online === true)).toBe(true);
});

test('connectivity: an account that needs re-authentication offers Reconnect', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await page.evaluate(() => {
    window.__siftFixture!.control.failAccount(
      window.__siftFixture!.ids.accountB,
      'auth',
      'Reconnect this account to continue syncing.',
    );
  });

  const rowB = page.getByTestId('connectivity-strip').locator('[data-account-id="acc-b"]');
  await expect(rowB).toHaveAttribute('data-state', 'reauth_required');
  await rowB.getByTestId('connectivity-reconnect').click();

  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeVisible();
});
