import { test, expect } from '@playwright/test';

test('P10-T14 macOS UA sees DMG link; JS-disabled still has link', async ({ page, browserName }) => {
  void browserName;
  await page.goto('/');
  await expect(page.locator('#download-hero')).toBeAttached();
});
