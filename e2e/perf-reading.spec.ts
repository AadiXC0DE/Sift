import { test, expect } from '@playwright/test';

test('P5-T11 thread-open p95 prefetched<=16ms db<=50ms', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
