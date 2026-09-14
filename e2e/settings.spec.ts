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

test('settings: mail row actions cannot paint through the modal', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  await app.row('blue-00').click();
  const action = page.locator('button[title="Archive (e)"]').first();
  await expect(action).toBeVisible();
  await page.keyboard.press('Meta+,');
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(dialog).toBeVisible();
  // Move the underlying actions beneath the centre of the opaque dialog.
  // Their pixels must make no difference to the rendered modal.
  const box = await dialog.boundingBox();
  expect(box).not.toBeNull();
  await action.evaluate((el, rect) => {
    const layer = el.parentElement!.parentElement!.parentElement!;
    layer.style.position = 'fixed';
    layer.style.top = `${rect!.y + 60}px`;
    layer.style.left = `${rect!.x + 40}px`;
    layer.style.right = 'auto';
    layer.style.width = '150px';
  }, box);
  const clip = { x: box!.x + 40, y: box!.y + 60, width: 150, height: 40 };
  const visible = await page.screenshot({ clip, animations: 'disabled' });
  await action.evaluate((el) => {
    el.parentElement!.style.visibility = 'hidden';
  });
  const hidden = await page.screenshot({ clip, animations: 'disabled' });
  expect(visible.equals(hidden)).toBe(true);
});
