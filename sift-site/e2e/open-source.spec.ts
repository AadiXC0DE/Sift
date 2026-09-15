import { test, expect } from '@playwright/test';

const PAGES = ['/', '/download', '/faq', '/privacy', '/benchmarks', '/changelog', '/shortcuts'];

// The nav used to hide every link below 960px, which left phones with no way to
// reach the source at all. The GitHub button has to survive every width.
for (const width of [320, 390, 768, 1440]) {
  test(`the source is reachable from the nav at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    const github = page.locator('#github-nav');
    await expect(github).toBeVisible();
    await expect(github).toHaveAttribute('href', 'https://github.com/AadiXC0DE/Sift');
    // An icon-only button still needs a name for screen readers.
    await expect(github).toHaveAttribute('aria-label', /GitHub/);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

test('every page names the license and links to its text', async ({ page }) => {
  for (const path of PAGES) {
    await page.goto(path);
    const license = page.locator('footer.site a[href$="/LICENSE"]');
    await expect(license, `${path} links the license`).toBeVisible();
    await expect(page.locator('footer.site')).toContainText('MIT');
    await expect(page.locator('footer.site a', { hasText: 'GitHub' })).toHaveAttribute(
      'href',
      'https://github.com/AadiXC0DE/Sift',
    );
  }
});

test('the landing page states the license beside its download call to action', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#open-source')).toContainText('MIT');
  await expect(page.locator('#github-hero')).toHaveAttribute('href', 'https://github.com/AadiXC0DE/Sift');
  await expect(page.locator('#github-oss')).toHaveAttribute('href', 'https://github.com/AadiXC0DE/Sift');
  await expect(page.locator('#github-cta')).toHaveAttribute('href', 'https://github.com/AadiXC0DE/Sift');
});

test('the nav download button stays a nav button on a phone', async ({ page }) => {
  // It grew to 142x40 with the full "Download for Mac" label, which crowded the
  // bar at 320px. Bound it, and keep it no taller than the button row it sits in.
  for (const width of [320, 390]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    const button = await page.locator('#download-nav').boundingBox();
    expect(button!.height).toBeLessThanOrEqual(40);
    expect(button!.width).toBeLessThanOrEqual(130);
    await expect(page.locator('#download-nav')).toContainText('Download');
  }
});
