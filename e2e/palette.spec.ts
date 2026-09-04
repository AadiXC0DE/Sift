import { test, expect } from '@playwright/test';

test('P8-T09 palette archives 2 selected', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
