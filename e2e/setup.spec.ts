import type { Page } from '@playwright/test';
import { expect, test } from './fixture/test';

async function toAppPasswordStep(page: Page): Promise<void> {
  await page.getByText('Connect your Gmail').click();
  await page.getByPlaceholder('you@gmail.com').fill('you@gmail.com');
  await page.getByText('Continue', { exact: true }).click();
  await expect(page.getByText('Get an app password')).toBeVisible({ timeout: 15_000 });
}

/**
 * P4.4/P2: a rejected app password must return the wizard to a corrective step
 * and must not create an account. The in-flight sign-in promise releases its
 * shared slot on both outcomes, so a rejection never surfaces as an unhandled
 * rejection and the corrective step renders.
 */
test('setup: a rejected app password shows a guided fix and creates no account', async ({ page, app }) => {
  await app.gotoApp({ scenario: 'empty' });
  await page.evaluate(() => window.__siftFixture!.control.failAppPasswordFlow(true));

  await toAppPasswordStep(page);
  await page.getByLabel('16-letter app password').fill('abcd efgh ijkl mnop');
  await page.getByText('Connect', { exact: true }).click();

  await expect(page.getByText(/Back to app password|Try again/).first()).toBeVisible({ timeout: 20_000 });
  const calls = await page.evaluate(() => window.__siftFixture!.control.calls());
  expect(calls.some((c) => c.cmd === 'accounts_add_app_password')).toBe(true);
  const accounts = await page.evaluate(() => window.__siftFixture!.control.accounts());
  expect(accounts).toHaveLength(0);
  await expect(page.locator('[data-testid^="row-"]')).toHaveCount(0);
});

/**
 * P11-T20: reduced motion must collapse the wizard's step transitions to zero.
 */
test('setup: reduced motion collapses the wizard transitions', async ({ page, app }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await app.gotoApp({ scenario: 'empty' });
  await expect(page.getByText('Connect your Gmail')).toBeVisible({ timeout: 15_000 });

  const longest = await page.evaluate(() => {
    let max = 0;
    for (const el of document.querySelectorAll<HTMLElement>('#root *')) {
      const cs = getComputedStyle(el);
      for (const v of `${cs.transitionDuration},${cs.animationDuration}`.split(',')) {
        max = Math.max(max, parseFloat(v) || 0);
      }
    }
    return max;
  });
  expect(longest).toBeLessThanOrEqual(0.001);
});
