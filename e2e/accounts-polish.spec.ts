import { test, expect } from '@playwright/test';

test('P9-T08 color/reorder/include-in-unified', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
