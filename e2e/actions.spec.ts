import { expect, test } from './fixture/test';
import {
  fixtureCalls,
  persistedThread,
  pressList,
  revealRow,
  showsUnreadDot,
  sidebarLabel,
  sidebarView,
} from './fixture/helpers';

test('archive: the row leaves Inbox, persists as archived and stays gone after a reload', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await app.row('blue-01').click();
  await pressList(page, 'e');

  await expect(app.row('blue-01')).toHaveCount(0);
  expect((await persistedThread(page, 'blue-01')).labelIds).not.toContain('INBOX');

  // The persisted state is what a reload sees, not the component's memory.
  await page.reload();
  await app.ready();
  await expect(app.row('blue-01')).toHaveCount(0);

  await sidebarView(page, 'Archive');
  await expect(app.row('blue-01')).toHaveCount(1);
});

test('bulk trash: three selected threads move to Trash and Undo restores them to Inbox', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  // One account only: a multi-account gesture currently yields one operation
  // group per account (P6.3), which is a separate concern from this test.
  await app.row('blue-01').click();
  await pressList(page, 'x');
  await pressList(page, 'j');
  await pressList(page, 'x');
  await pressList(page, 'j');
  await pressList(page, 'x');
  await expect(page.getByText('3 selected')).toBeVisible();

  await pressList(page, '#');
  await expect(app.row('blue-01')).toHaveCount(0);
  for (const id of ['blue-01', 'blue-02', 'blue-03']) {
    expect((await persistedThread(page, id)).labelIds).toContain('TRASH');
  }

  // The toast's Undo is the user gesture for this action; it targets exactly the
  // operation group the trash dispatch created.
  const trashToast = page
    .locator('[data-sonner-toast]', { hasText: '3 conversations moved to Trash' })
    .first();
  await expect(trashToast).toBeVisible();
  await trashToast.getByRole('button', { name: 'Undo (z)' }).click();
  await expect
    .poll(async () => (await persistedThread(page, 'blue-01')).labelIds.includes('TRASH'))
    .toBe(false);
  for (const id of ['blue-01', 'blue-02', 'blue-03']) {
    expect((await persistedThread(page, id)).labelIds).toContain('INBOX');
  }

  await sidebarView(page, 'Trash');
  await expect(app.row('blue-01')).toHaveCount(0);
  await sidebarView(page, 'Inbox');
  await expect(app.row('blue-01')).toHaveCount(1);
});

test('star: the toggle is visible in the Starred view and survives a reload', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await app.row('blue-01').click();
  await pressList(page, 's');
  await expect(page.locator('h1')).toBeVisible();

  await sidebarView(page, 'Starred');
  await expect(app.row('blue-01')).toHaveCount(1);
  expect((await persistedThread(page, 'blue-01')).isStarred).toBe(true);

  await page.reload();
  await app.ready();
  await sidebarView(page, 'Starred');
  await expect(app.row('blue-01')).toHaveCount(1);

  // Toggling it off removes it from the Starred view.
  await app.row('blue-01').click();
  await pressList(page, 's');
  await expect(app.row('blue-01')).toHaveCount(0);
});

test('read: marking a row read clears its unread dot and lowers the inbox count', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  // blue-00 is auto-opened by the reader and marked read; blue-03 is unread and
  // untouched, so it is the row this test acts on.
  const row = app.row('blue-03');
  expect(await showsUnreadDot(row)).toBe(true);
  const inbox = page.locator('nav[aria-label="Mailbox"]').getByRole('button', { name: /^Inbox/ });
  await expect(inbox).toContainText('21');

  await row.click();
  await pressList(page, 'Shift+i');

  await expect.poll(async () => (await persistedThread(page, 'blue-03')).unreadCount).toBe(0);

  await page.reload();
  await app.ready();
  expect(await showsUnreadDot(app.row('blue-03'))).toBe(false);
  // 21 seeded unread - blue-00 (auto-opened) - blue-03.
  await expect(inbox).toContainText('19');
});

test('label: adding Client Work exposes the thread in that label view', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await app.row('blue-01').click();
  await pressList(page, 'l');

  const item = page.locator('[cmdk-item]', { hasText: 'Client Work' }).first();
  await expect(item).toBeVisible();
  await item.click();

  await expect
    .poll(async () => (await persistedThread(page, 'blue-01')).labelIds.includes('Label_A_Client'))
    .toBe(true);

  await sidebarLabel(page, 'Client Work', 'acc-a');
  await expect(app.row('blue-01')).toHaveCount(1);
  await expect(app.row('blue-00')).toHaveCount(0); // not labelled
  // Every thread carrying that label id is listed.
  await expect(await revealRow(page, 'mixed-33')).toHaveCount(1);
});

test('label: the account-qualified label id is what the query uses, not the display name', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();
  await sidebarLabel(page, 'Client Work', 'acc-a');
  await expect.poll(async () => (await fixtureCalls(page, 'threads_query')).length).toBeGreaterThan(1);
  const queries = await fixtureCalls(page, 'threads_query');
  const view = (queries[queries.length - 1].query as { view: { kind: string; labelId?: string } }).view;
  expect(view.kind).toBe('label');
  expect(view.labelId).toBe('Label_A_Client');
});
