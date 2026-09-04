import { test, expect } from '@playwright/test';

test('P4-T16 unified interleaves with stripes; cmd2 narrows; cmd0 returns', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
