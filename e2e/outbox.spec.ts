import type { Page } from '@playwright/test';
import { expect, test } from './fixture/test';
import { fixtureCalls, outbox } from './fixture/helpers';

async function openComposer(page: Page): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('c');
  await expect(page.getByLabel('To recipients')).toBeVisible();
}

/** Queue one real send so the panel has an operation with a known subject. */
async function queueSend(page: Page, subject: string): Promise<number> {
  await openComposer(page);
  await page.getByLabel('To recipients').fill('ben@example.test');
  await page.keyboard.press('Enter');
  await page.getByLabel('Subject').fill(subject);
  await page.keyboard.press('Meta+Enter');
  await expect(page.getByTestId('compose-queued')).toContainText('Queued · Undo');
  await page.getByRole('button', { name: 'Close composer' }).click();
  return page.evaluate(() => window.__siftFixture!.control.outbox()[0].op_id);
}

async function openPanel(page: Page): Promise<void> {
  await page.getByTestId('outbox-indicator').click();
  await expect(page.getByTestId('outbox-panel')).toBeVisible();
}

/**
 * P6.6: the sidebar's pending indicator opens a compact Outbox, one row per
 * operation, and an uncertain send cannot be retried without the user stating
 * the duplicate risk.
 */
test('P6.6 the pending indicator opens the Outbox and an uncertain send needs an acknowledgement', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  const opId = await queueSend(page, 'Needs confirmation');

  await page.evaluate(([id]) => window.__siftFixture!.control.setOpState(id as number, 'uncertain'), [opId]);

  await openPanel(page);
  const row = page.getByTestId(`outbox-row-${opId}`);
  await expect(row).toBeVisible();
  await expect(row).toHaveAttribute('data-state', 'uncertain');
  await expect(row).toContainText('Needs confirmation');

  const retry = page.getByTestId(`outbox-retry-${opId}`);
  await expect(retry).toBeDisabled();
  await retry.click({ force: true });
  expect(await fixtureCalls(page, 'outbox_retry')).toHaveLength(0);

  await page.getByTestId(`outbox-ack-${opId}`).check();
  await expect(retry).toBeEnabled();
  await retry.click();
  await expect.poll(async () => (await outbox(page))[0].state).toBe('pending');

  // The acknowledgement belongs to the operation and is cleared when the panel
  // closes: a fresh uncertain state asks for it again rather than remembering
  // a consent the user cannot see.
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('outbox-panel')).toBeHidden();
  await page.evaluate(([id]) => window.__siftFixture!.control.setOpState(id as number, 'uncertain'), [opId]);
  await openPanel(page);
  await expect(page.getByTestId(`outbox-retry-${opId}`)).toBeDisabled();
});

/**
 * P6.2: the pending row is durable, so a restart still shows the operation the
 * user has not decided about yet.
 */
test('P6.2 the queued operation is still visible after a restart', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  const opId = await queueSend(page, 'Survives a restart');

  await page.reload();
  await app.ready();

  await expect(page.getByTestId('outbox-indicator')).toContainText('1');
  await openPanel(page);
  const row = page.getByTestId(`outbox-row-${opId}`);
  await expect(row).toBeVisible();
  await expect(row).toContainText('Survives a restart');
  await expect(row).toHaveAttribute('data-state', 'pending');
});

test('P6.6 a failed operation is retryable without an acknowledgement', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  const opId = await queueSend(page, 'Rejected once');
  await page.evaluate(([id]) => window.__siftFixture!.control.setOpState(id as number, 'failed'), [opId]);

  await openPanel(page);
  const row = page.getByTestId(`outbox-row-${opId}`);
  await expect(row).toHaveAttribute('data-state', 'failed');
  await expect(row).toContainText('The provider rejected this message');

  await page.getByTestId(`outbox-retry-${opId}`).click();
  await expect.poll(async () => (await outbox(page))[0].state).toBe('pending');
});

/**
 * P6.6: a 10,000-operation queue stays a small panel. The page renders one
 * page of rows, reports the real total, and never puts a payload on screen.
 */
test('P6.6 a 10,000-operation queue stays one page and never shows payload content', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await page.evaluate(() => window.__siftFixture!.control.seedOutbox(10_000));
  await expect(page.getByTestId('outbox-indicator')).toContainText('10,000');

  await openPanel(page);
  await expect(page.getByText('Showing 50 of 10,000')).toBeVisible();
  await expect(page.locator('[data-testid^="outbox-row-"]')).toHaveCount(50);

  const text = await page.getByTestId('outbox-panel').innerText();
  expect(text).not.toContain('raw-mime');
  expect(text.toLowerCase()).not.toContain('content-transfer-encoding');
});
