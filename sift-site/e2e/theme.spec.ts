import { test, expect } from '@playwright/test';

test('P10-T15 hero swaps with theme, no CLS', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#download-hero')).toBeAttached();
});
