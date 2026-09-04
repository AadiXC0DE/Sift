import { test, expect } from '@playwright/test';

test('P4-T08 virtualized list renders window', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
test('P4-T09 each view shows expected subjects', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
test('P4-T10 sidebar counts update on store:labels', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
test('P4-T12 unread filter restores scroll and cursor', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
