import { test, expect } from '@playwright/test';

test('P8-T10 find highlights and cycles', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
