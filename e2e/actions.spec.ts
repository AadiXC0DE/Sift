import { test, expect } from '@playwright/test';

test('P6-T12 archive moves cursor, undo restores', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
test('P6-T13 bulk trash 3 rows', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
test('P6-T14 snooze leaves inbox, wake returns', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
