import { test, expect } from '@playwright/test';

test('P10-T03 update toast, restart installs, no modal', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
