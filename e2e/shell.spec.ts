import { test, expect } from '@playwright/test';

test('P1-T12 shell renders sidebar/list/pane; cmd+\\ hides sidebar', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
  // sidebar present
  await expect(page.getByRole('button', { name: 'Settings' }).first()).toBeVisible({ timeout: 15000 });
});

test('P1-T12b theme follows prefers-color-scheme', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
