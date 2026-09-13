import { expect, test } from './fixture/test';

/**
 * P9-T10: reduced motion must collapse the shell's transitions and animations
 * to (effectively) zero, so the app honours the system setting.
 */
test('motion: reduced-motion collapses row transitions and animations', async ({ page, app }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await app.gotoApp();
  await app.ready();

  const durations = await page.evaluate(() => {
    const row = document.querySelector<HTMLElement>('[data-testid^="row-"]');
    if (!row) throw new Error('no row rendered');
    const cs = getComputedStyle(row);
    const parse = (v: string) => Math.max(...v.split(',').map((p) => parseFloat(p) || 0));
    return { transition: parse(cs.transitionDuration), animation: parse(cs.animationDuration) };
  });

  expect(durations.transition).toBeLessThanOrEqual(0.001);
  expect(durations.animation).toBeLessThanOrEqual(0.001);
});

test('motion: without reduced motion the row transition is not collapsed', async ({ page, app }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await app.gotoApp();
  await app.ready();

  const transition = await page
    .locator('[data-testid^="row-"]')
    .first()
    .evaluate((el) => getComputedStyle(el).transitionDuration);
  expect(parseFloat(transition)).toBeGreaterThan(0.001);
});
