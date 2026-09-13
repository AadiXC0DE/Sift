import { expect, test } from './fixture/test';
import { loadedRowCount, listScrollTop, scrollListTo, topVisibleRowId } from './fixture/helpers';

const ROW_H = 40;

/**
 * UI-03 (P3.3/P3.4): the loaded window is a scroll-window, not "page one". A
 * head refresh must keep the loaded rows, and a new head row must be queryable.
 */
test('UI-03 new mail at the head keeps the loaded 400-row window', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  for (let i = 0; i < 5; i++) await scrollListTo(page, 1);
  const loaded = await loadedRowCount(page, ROW_H);
  expect(loaded).toBeGreaterThanOrEqual(400);
  expect(loaded).toBeLessThanOrEqual(500);

  await scrollListTo(page, 0.5);
  await page.evaluate(() =>
    window.__siftFixture!.control.injectNewMail('acc-a', 'newest-0', 'Newest arrival'),
  );
  await page.waitForTimeout(400);

  // The refresh replaces the window in place: still 400 rows, no page-one-only
  // collapse, and the anchor row is still loaded.
  expect(await loadedRowCount(page, ROW_H)).toBeGreaterThanOrEqual(400);
  await expect
    .poll(async () => (await app.rowIds()).length, { message: 'list still renders rows' })
    .toBeGreaterThan(0);

  await scrollListTo(page, 0);
  await expect(app.row('newest-0')).toHaveCount(1);
});

/**
 * UI-03 (P3.4/P3.5) — expected failure. When a refresh changes the focused
 * row's index, or removes the focused row entirely, the "keep focused visible"
 * effect calls `virtual.scrollToIndex(focusedIndex)`. With a new head row the
 * focused index shifts by one and with an archived focused row it falls back to
 * 0, so the viewport jumps to the top instead of preserving the anchor the
 * refresh logic computes. Remove `test.fail()` when the focus-follow effect
 * stops overriding the anchor restore.
 */
test('UI-03 a head refresh and archiving keep the scroll anchor', async ({ page, app }) => {
  test.fail();
  await app.gotoApp();
  await app.ready();
  for (let i = 0; i < 5; i++) await scrollListTo(page, 1);
  await scrollListTo(page, 0.5);

  const anchor = await topVisibleRowId(page);
  const scrollBefore = await listScrollTop(page);
  expect(anchor).not.toBe('');

  await page.evaluate(() =>
    window.__siftFixture!.control.injectNewMail('acc-a', 'newest-0', 'Newest arrival'),
  );
  await page.waitForTimeout(400);
  expect(await topVisibleRowId(page)).toBe(anchor);
  expect(Math.abs((await listScrollTop(page)) - scrollBefore)).toBeLessThan(ROW_H * 2);

  const targetId =
    (await page.locator('[role="listbox"] [data-testid^="row-"]').nth(3).getAttribute('data-testid')) ?? '';
  const threadId = targetId.replace(/^row-/, '');
  await app.row(threadId).click();
  await page.keyboard.press('e');
  await expect(app.row(threadId)).toHaveCount(0);
  expect(await listScrollTop(page)).toBeGreaterThan(scrollBefore - ROW_H * 2);
});

test('UI-03 density change applies the new row height and keeps the top row in view', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();
  for (let i = 0; i < 3; i++) await scrollListTo(page, 1);
  await scrollListTo(page, 0.5);

  const topBefore = await topVisibleRowId(page);
  expect(topBefore).not.toBe('');

  await page.keyboard.press('Meta+,');
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Appearance', exact: true }).click();
  await dialog.getByRole('button', { name: 'Comfortable', exact: true }).click();
  await dialog.getByRole('button', { name: 'Close' }).click();
  await expect(dialog).toBeHidden();
  await page.waitForTimeout(250);

  expect(await app.row(topBefore).evaluate((el) => el.getBoundingClientRect().height)).toBe(48);
  // The renderer and the virtualizer share one row-height token, and the row
  // that was at the top of the viewport is still on screen.
  await expect(app.row(topBefore)).toBeVisible();
  expect(await loadedRowCount(page, 48)).toBeGreaterThanOrEqual(300);
});
