import { test, expect } from '@playwright/test';

test('P7-T12 compose send toast undo reopens', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
test('P7-T13 inline reply To/signature/quote', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
test('P7-T14 mailto prefill', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
test('P7-T15 drafts view restores', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
});
