import type { Page } from '@playwright/test';
import { expect, test } from './fixture/test';
import { drafts } from './fixture/helpers';

async function openComposer(page: Page): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('c');
  await expect(page.getByLabel('To recipients')).toBeVisible();
}

/** Compose one draft and leave it stored. */
async function composeDraft(page: Page, subject: string, to: string, body: string): Promise<void> {
  await openComposer(page);
  const recipients = page.getByLabel('To recipients');
  await recipients.fill(to);
  await recipients.press('Enter');
  await page.getByLabel('Subject').fill(subject);
  const editor = page.locator('.tiptap').first();
  await editor.click();
  await editor.pressSequentially(body);
  await page.getByRole('button', { name: 'Close composer' }).click();
  await expect(page.getByLabel('Subject')).toHaveCount(0);
  await expect.poll(async () => (await drafts(page)).length).toBe(1);
}

async function openDraftsView(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Drafts' }).click();
  await expect(page.getByText('Drafts', { exact: true }).first()).toBeVisible();
}

/**
 * P5.2: the Drafts view is a real list of stored drafts, and clicking one
 * reopens the composer on that draft — never on a new identity.
 */
test('P5.2 a stored draft is listed and reopens in place with the same id', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  await composeDraft(page, 'Draft from the view', 'ada@example.test', 'body of the draft');

  const stored = (await drafts(page))[0];
  await openDraftsView(page);

  const row = page.getByTestId('draft-row').first();
  await expect(row).toBeVisible();
  await expect(row).toContainText('Draft from the view');
  await expect(row).toContainText('ada@example.test');

  await row.click();

  await expect(page.getByLabel('Subject')).toHaveValue('Draft from the view');
  await expect(page.getByLabel('Remove ada@example.test')).toBeVisible();
  await expect(page.locator('.tiptap').first()).toContainText('body of the draft');

  // Editing the reopened draft updates the same row instead of creating one.
  await page.getByLabel('Subject').fill('Draft from the view, edited');
  await expect.poll(async () => (await drafts(page))[0].subject).toBe('Draft from the view, edited');
  const after = await drafts(page);
  expect(after).toHaveLength(1);
  expect(after[0].localId).toBe(stored.localId);
  expect(after[0].revision).toBeGreaterThan(stored.revision);
});

/** P5.2 / SEND-01: a restart restores every field the draft was saved with. */
test('P5.2 a restart restores the draft fields and its attachment', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  await openComposer(page);
  const recipients = page.getByLabel('To recipients');
  await recipients.fill('ada@example.test');
  await recipients.press('Enter');
  await page.getByLabel('Subject').fill('Survives a restart');
  const editor = page.locator('.tiptap').first();
  await editor.click();
  await editor.pressSequentially('restored body');
  await page.getByTitle('Attach (⌘⇧A)').click();
  await expect(page.getByLabel('Remove attached-report.pdf')).toBeVisible();
  await page.getByRole('button', { name: 'Close composer' }).click();
  await expect.poll(async () => (await drafts(page)).length).toBe(1);

  await page.reload();
  await app.ready();
  await openDraftsView(page);
  await page.getByTestId('draft-row').first().click();

  await expect(page.getByLabel('Subject')).toHaveValue('Survives a restart');
  await expect(page.getByLabel('Remove ada@example.test')).toBeVisible();
  await expect(page.getByLabel('Remove attached-report.pdf')).toBeVisible();
  await expect(page.locator('.tiptap').first()).toContainText('restored body');
});

/** The Drafts view says so plainly when there is nothing stored. */
test('P5.2 an empty Drafts view is empty, not an error', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  await openDraftsView(page);

  await expect(page.getByText('No drafts')).toBeVisible();
  await expect(page.getByText(/could not load/i)).toHaveCount(0);
});
