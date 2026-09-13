import type { Page } from '@playwright/test';
import { expect, test } from './fixture/test';
import { drafts, outbox } from './fixture/helpers';

async function openComposer(page: Page): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('c');
  await expect(page.getByLabel('To recipients')).toBeVisible();
}

async function typeBody(page: Page, text: string): Promise<void> {
  const editor = page.locator('.tiptap').first();
  await editor.click();
  await editor.pressSequentially(text);
}

/**
 * SEND-01 (P5.1): a subject-only edit followed by an immediate close must leave
 * one durable draft carrying the latest fields — the close path has to flush
 * the pending snapshot rather than wait for a body edit.
 */
test('SEND-01 subject-only edit then immediate close persists one draft with the subject', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await openComposer(page);

  await page.getByLabel('Subject').fill('Only a subject here');
  await page.getByRole('button', { name: 'Close composer' }).click();
  await expect(page.getByLabel('Subject')).toHaveCount(0);

  await expect.poll(async () => (await drafts(page)).length).toBe(1);
  const saved = (await drafts(page))[0];
  expect(saved.subject).toBe('Only a subject here');
  expect(saved.state).toBe('editing');

  // Durable: a reload reads the same stored draft back from the backend.
  await page.reload();
  await app.ready();
  expect((await drafts(page))[0].subject).toBe('Only a subject here');
});

test('compose: a body edit is autosaved and survives close and reload', async ({ page, app }) => {
  await app.gotoApp();
  await openComposer(page);

  await page.getByLabel('Subject').fill('Draft survives');
  await typeBody(page, 'body text');
  await page.waitForTimeout(450); // 300ms autosave debounce

  await expect.poll(async () => (await drafts(page)).length).toBe(1);
  const saved = (await drafts(page))[0];
  expect(saved.subject).toBe('Draft survives');
  expect(saved.bodyHtml).toContain('body text');

  await page.getByRole('button', { name: 'Close composer' }).click();
  await page.reload();
  await app.ready();
  const after = (await drafts(page))[0];
  expect(after.subject).toBe('Draft survives');
  expect(after.bodyHtml).toContain('body text');
});

test('compose: send queues an undoable operation and Undo returns the draft to editing', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await openComposer(page);

  await page.getByLabel('To recipients').fill('ben@example.test');
  await page.keyboard.press('Enter');
  await page.getByLabel('Subject').fill('Queued mail');
  await typeBody(page, 'hello');
  await page.keyboard.press('Meta+Enter');

  const toast = page.getByText('Sent · Undo');
  await expect(toast).toBeVisible({ timeout: 5000 });

  const queued = await outbox(page);
  expect(queued).toHaveLength(1);
  expect(queued[0].state).toBe('pending');
  expect(queued[0].subject).toBe('Queued mail');

  await page.getByRole('button', { name: 'Undo' }).click();
  await expect.poll(async () => (await outbox(page))[0].state).toBe('cancelled');
  const restored = (await drafts(page)).find((d) => d.subject === 'Queued mail');
  expect(restored?.state).toBe('editing');
});
