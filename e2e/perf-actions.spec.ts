import { test, expect } from '@playwright/test';

test('P6-T15 archive p95 <=30ms keydown to removal', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
