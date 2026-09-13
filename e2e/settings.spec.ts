import type { Page } from '@playwright/test';
import { expect, test } from './fixture/test';

async function openSetting(page: Page, tab: string, control: string): Promise<void> {
  await page.keyboard.press('Meta+,');
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: tab, exact: true }).click();
  await dialog.getByRole('button', { name: control, exact: true }).click();
  await dialog.getByRole('button', { name: 'Close' }).click();
  await expect(dialog).toBeHidden();
}

test('settings: density selection changes the rendered row height and persists', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  expect(await app.row('blue-00').evaluate((el) => el.getBoundingClientRect().height)).toBe(40);

  await openSetting(page, 'Appearance', 'Compact');
  expect(await app.row('blue-00').evaluate((el) => el.getBoundingClientRect().height)).toBe(32);

  await page.reload();
  await app.ready();
  expect(await app.row('blue-00').evaluate((el) => el.getBoundingClientRect().height)).toBe(32);
  await expect(page.locator('html')).toHaveAttribute('data-density', 'compact');
});

test('settings: theme selection applies to the document and persists', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');

  await openSetting(page, 'Appearance', 'Dark');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  await page.reload();
  await app.ready();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});
