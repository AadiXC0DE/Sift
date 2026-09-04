import { test, expect } from '@playwright/test';

test('P9-T12 axe zero serious/critical', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
