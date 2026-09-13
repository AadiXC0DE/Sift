import { expect, test } from './fixture/test';

test('accounts: no-accounts scenario shows the onboarding wizard, not an empty shell', async ({
  page,
  app,
}) => {
  await app.gotoApp({ scenario: 'empty' });

  await expect(page.getByText('Connect your Gmail')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('[data-testid^="row-"]')).toHaveCount(0);
});

test('accounts: completing the app-password flow adds the account and leaves onboarding', async ({
  page,
  app,
}) => {
  await app.gotoApp({ scenario: 'empty' });

  await page.getByText('Connect your Gmail').click();
  await page.getByPlaceholder('you@gmail.com').fill('you@gmail.com');
  await page.getByText('Continue', { exact: true }).click();

  await expect(page.getByText('Get an app password')).toBeVisible({ timeout: 15_000 });
  await page.getByLabel('16-letter app password').fill('abcd efgh ijkl mnop');
  await page.getByText('Connect', { exact: true }).click();

  await expect(page.locator('nav[aria-label="Mailbox"]')).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText('Connect your Gmail')).toHaveCount(0);
  await expect(page.getByText('you@gmail.com').first()).toBeVisible();
  const added = await page.evaluate(() => window.__siftFixture!.control.calls());
  expect(added.some((c) => c.cmd === 'accounts_add_app_password')).toBe(true);
});
