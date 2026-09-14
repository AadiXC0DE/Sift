import { test, expect } from '@playwright/test';

test('download links agree across the landing page and install guide', async ({ page }) => {
  await page.goto('/');
  const href = await page.locator('#download-hero').getAttribute('href');
  expect(href).toBeTruthy();
  await expect(page.locator('#download-nav')).toHaveAttribute('href', href!);
  await expect(page.locator('#download-cta')).toHaveAttribute('href', href!);
  if (process.env.SIFT_EXPECTED_DMG) expect(href).toBe(process.env.SIFT_EXPECTED_DMG);
  await page.goto('/download');
  await expect(page.locator(`a[href="${href}"]`).first()).toBeVisible();
});

test('download remains available without JavaScript', async ({ browser, baseURL }) => {
  const context = await browser.newContext({ javaScriptEnabled: false, baseURL });
  const page = await context.newPage();
  await page.goto('/');
  await expect(page.locator('#download-hero')).toBeVisible();
  if (process.env.SIFT_EXPECTED_DMG) {
    await expect(page.locator('#download-hero')).toHaveAttribute('href', process.env.SIFT_EXPECTED_DMG);
  }
  await context.close();
});
