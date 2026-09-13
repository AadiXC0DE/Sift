import { expect, test } from './fixture/test';
import { revealRow } from './fixture/helpers';

/**
 * NATIVE-01 (P2/P9.3), browser-observable parts: the reader renders the
 * message, strips active markup, and routes attachment actions through the
 * native command surface. The native OS flows themselves cannot run here.
 */
test('reader: the selected thread renders its sanitized body in the message frame', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await expect(page.locator('h1')).toHaveText('Blue run 00');
  await expect(page.getByText('Ada Lovelace').first()).toBeVisible();

  const frame = page.frameLocator('iframe[title="Email"]');
  await expect(frame.locator('body')).toContainText('Blue run 00 body with the attachment set.');

  // The seeded message carries <script>window.__sift_xss=1</script>; it must
  // never execute inside the reader frame.
  expect(await page.evaluate(() => window.__sift_xss)).toBeUndefined();
});

test('reader: attachments are listed and Save As goes through the native save path', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await expect(page.getByText('invoice.pdf')).toBeVisible();
  await expect(page.getByText('notes.txt')).toBeVisible();

  await page.getByRole('button', { name: 'Save invoice.pdf' }).click();
  await expect(page.getByText('Saved invoice.pdf')).toBeVisible();

  const saved = await page.evaluate(() => window.__siftFixture!.control.savedAs());
  expect(saved.map((s) => s.filename)).toEqual(['invoice.pdf']);
});

test('reader: Open attachment asks the backend to hand the file to the system opener', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await page.getByTitle('Open invoice.pdf').click();
  await expect
    .poll(async () => (await page.evaluate(() => window.__siftFixture!.control.opened())).length)
    .toBe(1);

  const opened = await page.evaluate(() => window.__siftFixture!.control.opened());
  expect(opened[0]).toContain('att-pdf');
});

test('reader: choosing another thread replaces the displayed conversation immediately', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();
  await expect(page.locator('h1')).toHaveText('Blue run 00');

  await (await revealRow(page, 'mixed-00')).click();
  await expect(page.locator('h1')).toHaveText('Mixed 00');
  const frame = page.frameLocator('iframe[title="Email"]');
  await expect(frame.locator('body')).toContainText('body for mixed-00');
  await expect(page.locator('h1')).not.toHaveText('Blue run 00');
});
