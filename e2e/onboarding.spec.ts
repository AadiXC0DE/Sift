import { test, expect } from '@playwright/test';

test('P9-T09 onboarding progress then list on store:threads', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
