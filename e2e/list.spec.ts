import { expect, test } from './fixture/test';
import { fixtureCalls, loadedRowCount, scrollListTo, sidebarView } from './fixture/helpers';

const ROW_H = 40;

test('list: page one holds the 100 newest threads in (time, account, id) order', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  expect(await loadedRowCount(page, ROW_H)).toBe(100);
  // The seeded head is the 30-row blue run, newest first.
  expect(await app.row('blue-00').count()).toBe(1);
  const firstRows = await app.rowIds();
  expect(firstRows[0]).toBe('blue-00');
  expect(await app.row('needle-a-000').count()).toBe(0);

  const queries = await fixtureCalls(page, 'threads_query');
  expect(queries.length).toBeGreaterThanOrEqual(1);
  for (const q of queries) expect((q.query as { cursor?: string }).cursor).toBeUndefined();
});

test('list: the cursor appends the next page exactly once', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  expect(await loadedRowCount(page, ROW_H)).toBe(100);

  await scrollListTo(page, 1);
  await expect.poll(() => loadedRowCount(page, ROW_H), { message: 'second page must load' }).toBe(200);

  const cursors = (await fixtureCalls(page, 'threads_query'))
    .map((q) => (q.query as { cursor?: string }).cursor)
    .filter((c): c is string => typeof c === 'string');
  expect(cursors.length).toBeGreaterThanOrEqual(1);
  expect(new Set(cursors).size, 'the same cursor must not be requested twice').toBe(cursors.length);

  // Walk the loaded window: every loaded row renders at some point, and every
  // rendered thread id is distinct (page two is not page one again).
  const seen = new Set<string>();
  for (let i = 0; i < 40; i++) {
    await scrollListTo(page, i / 40);
    for (const id of await app.rowIds()) seen.add(id);
  }
  // Page two starts with the equal-timestamp needle threads (account id is part
  // of the sort tuple, so account B sorts first).
  expect(seen.has('needle-b-124')).toBe(true);
  expect(seen.size, 'all 200 loaded rows rendered exactly once').toBe(200);
});

test('list: every view shows only its own threads', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  await expect(app.row('blue-00')).toHaveCount(1);

  await sidebarView(page, 'Starred');
  await expect
    .poll(async () => (await app.rowIds()).every((id) => ['blue-02', 'blue-03'].includes(id)))
    .toBe(true);

  await sidebarView(page, 'Archive');
  await expect(app.row('archived-0')).toHaveCount(1);
  await expect(app.row('blue-00')).toHaveCount(0);

  await sidebarView(page, 'Trash');
  await expect(app.row('trashed-0')).toHaveCount(1);
  await expect(app.row('archived-0')).toHaveCount(0);

  await sidebarView(page, 'Spam');
  await expect(app.row('spam-0')).toHaveCount(1);

  await sidebarView(page, 'Inbox');
  await expect(app.row('blue-00')).toHaveCount(1);
  await expect(app.row('trashed-0')).toHaveCount(0);
});

test('list: the unread filter only returns unread threads', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  // Exact: a row can also carry an accessible "Mark unread" action (P9.5), and
  // the toolbar control's own name is exactly "Unread".
  await page.getByRole('button', { name: 'Unread', exact: true }).click();
  await expect.poll(async () => (await fixtureCalls(page, 'threads_query')).length).toBeGreaterThan(1);

  const queries = await fixtureCalls(page, 'threads_query');
  expect((queries[queries.length - 1].query as { unread_only?: boolean }).unread_only).toBe(true);

  // The unread set as the query saw it. Rows can only leave it during the walk
  // (the reader marks the auto-opened thread read), never join it.
  const unreadAtQuery = await page.evaluate(() =>
    window
      .__siftFixture!.control.threads()
      .filter((t) => t.unreadCount > 0)
      .map((t) => t.id),
  );
  expect(unreadAtQuery).not.toContain('fill-00'); // fill-00 is read in the seed

  const seen = new Set<string>();
  for (let i = 0; i <= 19; i++) {
    await scrollListTo(page, i / 20);
    for (const id of await app.rowIds()) seen.add(id);
  }
  expect(seen.size).toBeGreaterThan(0);
  expect(seen.has('fill-00')).toBe(false);
  for (const id of seen) expect(unreadAtQuery, `${id} rendered by the unread filter`).toContain(id);
});

test('list: the sidebar inbox count is the sum of the seeded unread inbox threads', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  const inbox = page.locator('nav[aria-label="Mailbox"]').getByRole('button', { name: /^Inbox/ });
  // 10 unread in the blue run + 1 snoozed (account A) + 10 interleaved (account B).
  await expect(inbox).toContainText('21');
});
