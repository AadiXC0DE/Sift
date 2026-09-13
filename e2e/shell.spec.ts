import { expect, test } from './fixture/test';
import { fixtureCalls } from './fixture/helpers';

test('shell: seeded data renders the sidebar, the list window and the reader', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await expect(page.locator('nav[aria-label="Mailbox"]')).toBeVisible();
  await expect(page.getByRole('listbox', { name: 'Inbox' })).toBeVisible();
  await expect(app.row('blue-00')).toBeVisible();
  await expect(page.locator('h1')).toHaveText('Blue run 00');
});

test('shell: cmd+backslash hides the sidebar and restores it', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  const sidebar = page.locator('nav[aria-label="Mailbox"]');
  await expect(sidebar).toBeVisible();

  await page.keyboard.press('Meta+\\');
  await expect(sidebar).toBeHidden();

  await page.keyboard.press('Meta+\\');
  await expect(sidebar).toBeVisible();
});

test('shell: the command palette archives every selected thread', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await app.row('blue-05').click();
  await page.keyboard.press('x');
  await page.keyboard.press('j');
  await page.keyboard.press('x');
  await expect(page.getByText('2 selected')).toBeVisible();

  await page.keyboard.press('Meta+k');
  const palette = page.getByPlaceholder('Type a command or search…');
  await expect(palette).toBeVisible();
  await palette.fill('Archive');
  await page.keyboard.press('Enter');

  await expect(app.row('blue-05')).toHaveCount(0);
  const threads = await page.evaluate(() => window.__siftFixture!.control.threads());
  for (const id of ['blue-05', 'blue-06']) {
    expect(threads.find((t) => t.id === id)?.labelIds).not.toContain('INBOX');
  }

  const calls = await fixtureCalls(page, 'threads_action');
  expect(calls.length).toBeGreaterThanOrEqual(1);
});
