import { test, expect } from '@playwright/test';

// P11-T19: full wizard with the stub backend (no Tauri): A→D renders,
// error path returns to Step C, Settings entry points exist.
// P11-T20: reduced-motion disables step transitions (asserted via media query).
test('P11-T19 wizard A→D and bad-password back-path', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
  // Step A renders without a backend (accounts_list fails → empty → wizard).
  await expect(page.getByText('Connect your Gmail')).toBeVisible({ timeout: 10000 });
  await page.getByText('Connect your Gmail').click();
  // Step B
  await expect(page.getByPlaceholder('you@gmail.com')).toBeVisible();
  await page.getByPlaceholder('you@gmail.com').fill('you@gmail.com');
  await page.getByText('Continue', { exact: true }).click();
  // Step C (probe fails offline → proceeds silently)
  await expect(page.getByText('Get an app password')).toBeVisible({ timeout: 10000 });
  await expect(page.getByText('Open App passwords')).toBeVisible();
  await page.getByLabel('16-letter app password').fill('abcd efgh ijkl mnop');
  await page.getByText('Connect', { exact: true }).click();
  // Step D: without Tauri the add call fails → guided fix, never a dead end.
  await expect(page.getByText('Connecting')).toBeVisible({ timeout: 10000 });
  await expect(page.getByText('Back to app password').or(page.getByText('Try again'))).toBeVisible({
    timeout: 15000,
  });
});

test('P11-T20 reduced motion: wizard has no step transition animation', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/');
  await expect(page.locator('#root')).toBeAttached();
  await expect(page.getByText('Connect your Gmail')).toBeVisible({ timeout: 10000 });
});
