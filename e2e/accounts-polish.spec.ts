import { expect, test } from './fixture/test';
import { revealRow } from './fixture/helpers';

/**
 * P3.1: the decorative marker is aria-hidden, so the account identity has to
 * travel in the row's accessible label/tooltip — and both must disappear when
 * the scope already identifies the account.
 */
test('accounts: unified rows expose the account identity, single-account scope drops it', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  const blue = app.row('blue-00');
  await expect(blue).toHaveAttribute('title', 'ada@example.test');
  await expect(blue).toContainText('Account: ada@example.test');

  await expect(await revealRow(page, 'mixed-00')).toHaveAttribute('title', 'ben@example.test');
  await expect(page.locator('[data-testid="account-marker"]').first()).toHaveAttribute('aria-hidden', 'true');

  await page
    .getByRole('tablist', { name: 'Accounts' })
    .getByTitle(/ben@example\.test/)
    .click();
  const row = await revealRow(page, 'mixed-00');
  await expect(row).not.toHaveAttribute('title', /.+/);
  await expect(row).not.toContainText('Account:');
});
