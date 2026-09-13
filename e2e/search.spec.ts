import type { Page } from '@playwright/test';
import { expect, test } from './fixture/test';
import { fixtureCalls, loadedRowCount, scrollListTo } from './fixture/helpers';

const ROW_H = 40;

async function searchFor(page: Page, q: string): Promise<void> {
  const input = page.getByRole('textbox', { name: 'Search' });
  await input.click();
  await input.fill(q);
}

/**
 * SEARCH-03 (P7.3): 251 threads share one timestamp across two accounts. A
 * cursor that only carries the timestamp returns page one again; a cursor that
 * carries (timestamp, account, id) returns every hit exactly once.
 */
test('SEARCH-03 equal-timestamp results page exactly once across three pages', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await searchFor(page, 'needle');
  await expect(page.getByRole('listbox', { name: /Results for/ })).toBeVisible();
  expect(await loadedRowCount(page, ROW_H)).toBe(100);

  await scrollListTo(page, 1);
  await expect.poll(() => loadedRowCount(page, ROW_H)).toBe(200);
  await scrollListTo(page, 1);
  await expect.poll(() => loadedRowCount(page, ROW_H)).toBe(251);

  const cursors = (await fixtureCalls(page, 'threads_query'))
    .filter((q) => (q.query as { view?: { kind?: string } })?.view?.kind === 'search')
    .map((q) => (q.query as { cursor?: string }).cursor);
  expect(cursors.filter(Boolean).length).toBeGreaterThanOrEqual(2);
  expect(new Set(cursors.filter(Boolean)).size).toBe(cursors.filter(Boolean).length);

  const seen = new Set<string>();
  for (let i = 0; i <= 20; i++) {
    await scrollListTo(page, i / 20);
    for (const id of await app.rowIds()) seen.add(id);
  }
  expect(seen.size, 'every equal-timestamp hit rendered exactly once').toBe(251);
  expect(seen.has('needle-a-000')).toBe(true);
  expect(seen.has('needle-b-124')).toBe(true);
});

test('SEARCH-03 local search returns only the seeded hits', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await searchFor(page, 'invoice');
  await expect(page.getByRole('listbox', { name: /Results for/ })).toBeVisible();

  const ids = await app.rowIds();
  expect(ids).toContain('mixed-33');
  expect(ids.length).toBeGreaterThan(0);
  for (const id of ids) expect(id.startsWith('fill-') || id === 'mixed-33').toBe(true);
});

test('search: Escape clears the query and stops reporting a pending search', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await page.evaluate(() => window.__siftFixture!.control.delay('search', 400));
  await searchFor(page, 'needle');
  await expect(page.getByRole('listbox', { name: /Results for/ })).toBeVisible();

  const input = page.getByRole('textbox', { name: 'Search' });
  await input.press('Escape');
  await expect(input).toHaveValue('');

  // Escape must not leave the "searching" state stuck: the list keeps its own
  // scope title rather than a stale pending flag.
  const calls = await fixtureCalls(page, 'search');
  await page.waitForTimeout(600);
  expect((await fixtureCalls(page, 'search')).length).toBe(calls.length);
});

/**
 * SEARCH-04 (P7.2) — expected failure. Clearing the field while a search is
 * pending must restore the previous scope/anchor and must not apply the cleared
 * query. `SearchInput` keeps its stash in a local ref and never calls the view
 * store's `stashForSearch`, so `restore()` is a no-op: the list stays on the
 * search view, and the debounce scheduled by the last keystroke still switches
 * the view to the cleared query. Remove `test.fail()` when the search state
 * moves into the store as P7.2 requires.
 */
test('SEARCH-04 clearing during a pending search restores the previous view', async ({ page, app }) => {
  test.fail();
  await app.gotoApp();
  await app.ready();

  const input = page.getByRole('textbox', { name: 'Search' });
  await input.click();
  await input.fill('needle');
  await input.fill(''); // cleared before the 40ms debounce can settle
  await page.waitForTimeout(400);

  await expect(page.getByRole('listbox', { name: 'Inbox' })).toBeVisible();
  await expect(app.row('needle-a-000')).toHaveCount(0);
});
