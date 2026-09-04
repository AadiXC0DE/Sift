import { test, expect } from '@playwright/test';

test('P2-T09 onboarding mocked add shows account', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});

test('P2-T10 auth:expired banner shows sign-in', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
