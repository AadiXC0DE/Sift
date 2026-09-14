import { expect, test } from './fixture/test';
import { fixtureCalls, pressList, revealRow, sidebarView } from './fixture/helpers';
import type { Locator, Page } from '@playwright/test';

/**
 * Walk the virtualized window from top to bottom, collecting every thread id
 * the list rendered. The window is taller than the viewport and pages in 100
 * rows at a time, so the step is well under one screen: a coarser jump would
 * leave rows that were never rendered in the collected set.
 */
async function renderedIds(page: Page): Promise<Set<string>> {
  const list: Locator = page.locator('[role="listbox"]');
  const seen = new Set<string>();
  let top = 0;
  let lastMax = -1;
  let bottomHits = 0;
  for (let i = 0; i < 120; i++) {
    await list.evaluate((el, t) => {
      el.scrollTop = t;
    }, top);
    await page.waitForTimeout(60);
    for (const id of await page.$$eval('[data-testid^="row-"]', (els) =>
      els.map((el) => (el.getAttribute('data-testid') ?? '').replace(/^row-/, '')),
    )) {
      seen.add(id);
    }
    const pos = await list.evaluate((el) => ({
      top: el.scrollTop,
      max: el.scrollHeight - el.clientHeight,
    }));
    if (pos.top >= pos.max - 1) {
      // At the bottom: the next page only arrives after this scroll settles.
      if (pos.max === lastMax && ++bottomHits > 2) break;
      lastMax = pos.max;
      await page.waitForTimeout(180);
    }
    top = pos.top + 320;
  }
  return seen;
}

/**
 * All Mail (P3.6) is a mailbox, not the account scope: every conversation with
 * a message outside Trash/Junk, across whichever accounts are in scope.
 */
test('all mail: inbox, archived and sent mail are listed; Trash and Junk are not', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await sidebarView(page, 'All Mail');
  await expect(page.getByRole('listbox', { name: 'All Mail' })).toBeVisible();

  // Inbox mail shows in Inbox and here.
  await expect(app.row('blue-00')).toHaveCount(1);

  const ids = await renderedIds(page);
  expect(ids.has('blue-00')).toBe(true);
  // Archived mail is not in Inbox but is still All Mail.
  expect(ids.has('archived-0')).toBe(true);
  expect(ids.has('sent-thread-0')).toBe(true);
  // Trash and Junk are excluded at message level.
  expect(ids.has('trashed-0')).toBe(false);
  expect(ids.has('spam-0')).toBe(false);
  expect(ids.size).toBeGreaterThan(400);
});

test('all mail: the view is not the account scope — narrowing to one account filters it', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await sidebarView(page, 'All Mail');
  await expect(await revealRow(page, 'mixed-00')).toHaveCount(1);

  await page
    .getByRole('tablist', { name: 'Accounts' })
    .getByTitle(/ada@example\.test/)
    .click();

  await expect(app.row('blue-00')).toHaveCount(1);
  const seen = await renderedIds(page);
  expect(seen.has('mixed-00'), 'account B rows must leave the All Mail window').toBe(false);
  expect(seen.has('archived-0')).toBe(true);

  const queries = await fixtureCalls(page, 'threads_query');
  const last = queries[queries.length - 1].query as { accountIds: string[]; view: { kind: string } };
  expect(last.view.kind).toBe('all_mail');
  expect(last.accountIds).toEqual(['acc-a']);
});

test('all mail: membership is per message — a half-trashed conversation is in All Mail, not Trash', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  // The conversation holds one inbox message and one trashed message.
  await sidebarView(page, 'Trash');
  await expect(app.row('trashed-0')).toHaveCount(1);
  await expect(app.row('mixed-thread-0')).toHaveCount(0);

  await sidebarView(page, 'Inbox');
  expect((await renderedIds(page)).has('mixed-thread-0')).toBe(true);

  await sidebarView(page, 'All Mail');
  expect((await renderedIds(page)).has('mixed-thread-0')).toBe(true);
});

test('labels: the same name in two accounts stays two labels with two ids', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  // Both accounts own "Client Work"; the sidebar must not merge them by name.
  const sidebar = page.locator('nav[aria-label="Mailbox"]');
  const forA = sidebar.locator('button[data-label-name="Client Work"][data-account-id="acc-a"]');
  const forB = sidebar.locator('button[data-label-name="Client Work"][data-account-id="acc-b"]');
  await expect(forA).toHaveCount(1);
  await expect(forB).toHaveCount(1);
  // The duplicate name is disambiguated on screen, not only in the tooltip.
  await expect(forA).toContainText('ada@example.test');
  await expect(forB).toContainText('ben@example.test');

  await forA.click();
  // The row chip shows the label's name, not its id.
  await expect(await revealRow(page, 'mixed-33')).toContainText('Client Work');
  let queries = await fixtureCalls(page, 'threads_query');
  let last = queries[queries.length - 1].query as { view: { kind: string; labelId?: string } };
  expect(last.view).toEqual({ kind: 'label', labelId: 'Label_A_Client' });

  await forB.click();
  queries = await fixtureCalls(page, 'threads_query');
  last = queries[queries.length - 1].query as { view: { kind: string; labelId?: string } };
  expect(last.view).toEqual({ kind: 'label', labelId: 'Label_B_Client' });
});

test('labels: a mutation never borrows the other account label id', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  // mixed-00 belongs to account B; its label picker must offer B's labels.
  await (await revealRow(page, 'mixed-00')).click();
  await pressList(page, 'l');

  const picker = page.locator('[cmdk-list] [cmdk-item]', { hasText: 'Client Work' }).first();
  await expect(picker).toBeVisible();
  await picker.click();

  await expect.poll(async () => (await fixtureCalls(page, 'threads_action')).length).toBeGreaterThan(0);
  const actions = await fixtureCalls(page, 'threads_action');
  const applied = actions
    .map(
      (c) =>
        c as unknown as {
          targets: { accountId: string; threadId: string }[];
          action: { kind: string; labelId?: string };
        },
    )
    .filter((r) => r.action.kind === 'addLabel');
  expect(applied).toHaveLength(1);
  expect(applied[0].targets.map((t) => t.accountId)).toEqual(['acc-b']);
  expect(applied[0].action.labelId).toBe('Label_B_Client');
});
