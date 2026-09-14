import { test, expect } from '@playwright/test';

for (const width of [320, 390, 768, 1440]) {
  test(`landing fits ${width}px and controls work`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    await expect(page.locator('#download-hero')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.getByRole('button', { name: 'Rose accent' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-accent', 'rose');
    await page.getByRole('button', { name: 'Compact', exact: true }).click();
    await expect(page.locator('#preview')).toHaveAttribute('data-density', 'compact');
    await page.locator('#preview').focus();
    await page.keyboard.press('j');
    await expect(page.locator('#preview-list .thread').nth(1)).toHaveClass(/open/);
    await page.screenshot({ path: `/private/tmp/sift-landing-${width}.png`, fullPage: true });
  });
}

test('benchmark bars animate into view and remain visible with reduced motion', async ({ page }) => {
  await page.goto('/benchmarks');
  const chart = page.locator('[data-chart]');
  await chart.scrollIntoViewIfNeeded();
  await expect(chart).toHaveClass(/in/);
  await expect(page.locator('.bar-fill.sift')).toHaveCSS('transform', 'matrix(1, 0, 0, 1, 0, 0)');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.reload();
  await expect(page.locator('.bar-fill.sift')).toHaveCSS('transform', 'none');
  expect(await page.locator('.bar-fill.sift').evaluate(el => el.getBoundingClientRect().width)).toBeGreaterThan(20);
});
