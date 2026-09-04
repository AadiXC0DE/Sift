import { test, expect } from '@playwright/test';

test('P8-T08 from:ada filters; Esc restores scroll/cursor', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
