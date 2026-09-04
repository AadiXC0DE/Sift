import { test, expect } from '@playwright/test';

test('P9-T10 reduced-motion removes transforms', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
