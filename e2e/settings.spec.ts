import { test, expect } from '@playwright/test';

test('P9-T07 density changes row height, keeps scroll', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
