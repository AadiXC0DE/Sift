import type { Page } from '@playwright/test';
import { expect, test } from './fixture/test';
import { drafts, fixtureCalls, outbox, persistedThread, pressList } from './fixture/helpers';

async function openComposer(page: Page): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('c');
  await expect(page.getByLabel('To recipients')).toBeVisible();
}

test('compose: footer buttons align and the send dropdown stays attached', async ({ page, app }) => {
  await app.gotoApp();
  await openComposer(page);
  const send = page.getByRole('button', { name: 'Send ⌘↵', exact: true });
  const dropdown = page.getByTestId('send-later-trigger');
  const controls = [
    send,
    dropdown,
    page.getByRole('button', { name: 'Send & archive', exact: true }),
    page.getByRole('button', { name: 'Attach', exact: true }),
  ];
  const boxes = await Promise.all(controls.map((control) => control.boundingBox()));
  for (const box of boxes) {
    expect(box).not.toBeNull();
    expect(box!.height).toBe(32);
    expect(Math.abs(box!.y - boxes[0]!.y)).toBeLessThan(1);
  }
  expect(Math.abs(boxes[1]!.x - (boxes[0]!.x + boxes[0]!.width))).toBeLessThanOrEqual(1);
});

/** Every archive gesture the app issued, so a dependent archive is visible. */
async function archiveCalls(page: Page): Promise<Record<string, unknown>[]> {
  const calls = await fixtureCalls(page, 'threads_action');
  return calls.filter((c) => (c.action as { kind?: string } | undefined)?.kind === 'archive');
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

  // Queueing is not sending (P6.2): the composer keeps saying Queued · Undo
  // until the provider accepts the message.
  const banner = page.getByTestId('compose-queued');
  await expect(banner).toContainText('Queued · Undo');

  const queued = await outbox(page);
  expect(queued).toHaveLength(1);
  expect(queued[0].state).toBe('pending');
  expect(queued[0].subject).toBe('Queued mail');

  await page.getByRole('button', { name: 'Undo' }).click();
  await expect.poll(async () => (await outbox(page))[0].state).toBe('cancelled');
  const restored = (await drafts(page)).find((d) => d.subject === 'Queued mail');
  expect(restored?.state).toBe('editing');
});

/**
 * P6.2 (SEND-04/SEND-05): the composer never upgrades a queued operation to
 * "Sent", a failure stays visible and retryable, and Send & archive never
 * archives the source thread on its own.
 */
test('P6.2 a failed send says Not sent, keeps the thread in the Inbox and retries', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  // Reply from the reader so the send has a source thread that could be archived.
  await pressList(page, 'r');
  await expect(page.getByLabel('To recipients')).toBeVisible();
  await typeBody(page, 'about the report');
  await page.keyboard.press('Meta+Shift+Enter');

  const banner = page.getByTestId('compose-queued');
  await expect(banner).toContainText('Queued · Undo');
  // Send & archive is a dependent operation, so nothing is archived yet. The
  // reader's own mark-read gesture is a different action and is allowed.
  expect(await archiveCalls(page)).toHaveLength(0);
  expect((await persistedThread(page, 'blue-00')).labelIds).toContain('INBOX');

  const op = (await outbox(page))[0];
  await page.evaluate((opId) => window.__siftFixture!.control.setOpState(opId, 'failed'), op.op_id);

  await expect(banner).toContainText('Not sent');
  await expect(banner).not.toContainText('Queued · Undo');
  // A failed send never archives the conversation it came from.
  expect((await persistedThread(page, 'blue-00')).labelIds).toContain('INBOX');
  expect(await archiveCalls(page)).toHaveLength(0);

  await banner.getByRole('button', { name: 'Retry' }).click();
  await expect.poll(async () => (await outbox(page))[0].state).toBe('pending');
});

test('P6.2 double Send creates exactly one operation for the revision', async ({ page, app }) => {
  await app.gotoApp();
  await openComposer(page);

  await page.getByLabel('To recipients').fill('ben@example.test');
  await page.keyboard.press('Enter');
  await page.getByLabel('Subject').fill('Only once');

  // The keyboard path is the one a fast double tap hits; the second attempt
  // happens while the first operation is still queued.
  await page.keyboard.press('Meta+Enter');
  await expect(page.getByTestId('compose-queued')).toContainText('Queued · Undo');
  await page.keyboard.press('Meta+Enter');

  await expect.poll(async () => (await outbox(page)).length).toBe(1);
  await expect.poll(async () => (await fixtureCalls(page, 'drafts_send')).length).toBe(1);
});

/**
 * P5.1: a failed local write is a storage failure with a retry — never an
 * "offline" claim — and the composer stays open until the draft is durable.
 */
test('P5.1 a failed draft save keeps the composer open until it is retried', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  await openComposer(page);

  // Fail the debounced write and the close flush behind it.
  await page.evaluate(() => window.__siftFixture!.control.failDraftSaves(3));
  await page.getByLabel('Subject').fill('Must not vanish');

  const status = page.getByText('Couldn’t save');
  await expect(status).toBeVisible({ timeout: 5000 });
  await expect(status).not.toContainText(/offline/i);
  await expect.poll(async () => (await drafts(page)).length).toBe(0);

  await page.getByRole('button', { name: 'Close composer' }).click();
  // The storage failure keeps the sheet, and the text, on screen.
  await expect(page.getByLabel('Subject')).toHaveValue('Must not vanish');

  await page.evaluate(() => window.__siftFixture!.control.failDraftSaves(0));
  await page.getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByText('Saved')).toBeVisible({ timeout: 5000 });

  await page.getByRole('button', { name: 'Close composer' }).click();
  await expect(page.getByLabel('Subject')).toHaveCount(0);
  const stored = await drafts(page);
  expect(stored).toHaveLength(1);
  expect(stored[0].subject).toBe('Must not vanish');
  expect(stored[0].state).toBe('editing');
});

/**
 * P5.4: a reply quotes the real body (not the list snippet) with sender and
 * date attribution, keeps a single subject prefix, and carries the parent
 * message id for threading.
 */
test('P5.4 reply quotes the full message body and threads to its parent', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await pressList(page, 'r');
  await expect(page.getByLabel('To recipients')).toBeVisible();

  await expect(page.getByLabel('Subject')).toHaveValue('Re: Blue run 00');
  const quote = page.locator('.tiptap details[data-sift-quote]').first();
  await expect(quote).toBeVisible();
  await expect(quote).toContainText('Blue run 00 body with the attachment set.');
  // The list snippet is not what a reply quotes.
  await expect(page.locator('.tiptap')).not.toContainText('Blue account snippet 0');
  await expect(quote.locator('summary')).toContainText('Ada Lovelace <ada@example.test>');

  await expect.poll(async () => (await drafts(page)).length).toBe(1);
  const stored = (await drafts(page))[0];
  expect(stored.mode).toBe('reply');
  expect(stored.inReplyToMessageId).toBe('blue-00-m1');
  expect(stored.subject).toBe('Re: Blue run 00');
  expect(stored.bodyHtml).toContain('Blue run 00 body with the attachment set.');
});
