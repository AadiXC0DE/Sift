import { expect, test } from './fixture/test';
import { fixtureCalls } from './fixture/helpers';
import type { Locator, Page } from '@playwright/test';

const PDF = 'acc-a:blue-00-m1:att-pdf';
const TXT = 'acc-a:blue-00-m1:att-txt';
const PNG = 'acc-a:blue-00-m1:att-png';
const UNNAMED = 'acc-a:blue-00-m1:att-unnamed';

function strip(page: Page): Locator {
  return page.getByTestId('attachment-strip');
}

function attachmentButton(page: Page, action: 'open' | 'save', id: string): Locator {
  return strip(page).locator(`[data-attachment-id="${id}"][data-attachment-focus^="${action}-"]`);
}

test('attachments: the strip keeps unnamed parts and lists inline images only on request', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  const ids = await strip(page)
    .locator('[data-attachment-focus^="open-"]')
    .evaluateAll((els) => els.map((el) => el.getAttribute('data-attachment-id')));
  // Named non-inline parts *and* the part the sender left unnamed.
  expect(ids).toEqual([PDF, TXT, UNNAMED]);
  // The inline image belongs in the email until the reader asks for it.
  expect(ids).not.toContain(PNG);

  await page.getByTestId('attachment-show-all').click();
  await expect
    .poll(async () =>
      strip(page)
        .locator('[data-attachment-focus^="open-"]')
        .evaluateAll((els) => els.map((el) => el.getAttribute('data-attachment-id'))),
    )
    .toContain(PNG);
});

test('attachments: Save All writes every non-inline part once and never clobbers', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await page.getByTestId('attachment-save-all').click();
  await expect(page.getByText('Saved 3 attachments')).toBeVisible();

  const first = await page.evaluate(() => window.__siftFixture!.control.savedPaths());
  expect(first).toHaveLength(3);
  expect(first[0]).toBe('invoice.pdf');
  expect(first[1]).toBe('notes.txt');
  // The unnamed part is saved under the name the backend derives for it.
  expect(first[2]).toMatch(/^attachment-[a-z0-9]{8}\.bin$/);

  await page.getByTestId('attachment-save-all').click();
  await expect
    .poll(async () => (await page.evaluate(() => window.__siftFixture!.control.savedPaths())).length)
    .toBe(6);

  const all = await page.evaluate(() => window.__siftFixture!.control.savedPaths());
  expect(new Set(all).size, 'a second Save All must not overwrite the first').toBe(all.length);
  expect(all[3]).toBe('invoice (2).pdf');
});

test('attachments: Save All reports failures and retries them', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await page.evaluate(() => window.__siftFixture!.control.failSaveAll('acc-a:blue-00-m1:att-unnamed'));
  await page.getByTestId('attachment-save-all').click();

  const result = page.getByTestId('attachment-save-all-result');
  await expect(result).toContainText('Saved 2 of 3');
  await expect(result).toContainText('attachment-');

  await page.evaluate(() => window.__siftFixture!.control.failSaveAll(null));
  await expect(page.getByText('Saved 3 attachments')).toHaveCount(0);
  await result.getByRole('button', { name: 'Try again' }).click();
  await expect(result).toHaveCount(0);

  // The retry re-runs Save All; the parts that already landed are written
  // beside the first copy rather than over it.
  const paths = await page.evaluate(() => window.__siftFixture!.control.savedPaths());
  expect(paths).toHaveLength(5);
  expect(new Set(paths).size).toBe(paths.length);
  expect(paths.some((p) => /^attachment-[a-z0-9]{8}\.bin$/.test(p))).toBe(true);
});

test('attachments: Space previews a PDF through the same key Save As uses', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  const open = attachmentButton(page, 'open', PDF);
  await open.focus();
  await page.keyboard.press(' ');
  await expect(page.getByTestId('attachment-preview')).toBeVisible();
  // The preview renders the system WebView's own viewer for the
  // account-qualified part — the same address the Save As path copies.
  await expect(page.getByTestId('attachment-preview-frame')).toHaveAttribute(
    'src',
    `sift-att://acc-a/blue-00-m1/${PDF}`,
  );

  // A preview is not a save and not an open.
  expect(await fixtureCalls(page, 'attachments_save_as')).toHaveLength(0);
  expect(await fixtureCalls(page, 'attachments_open')).toHaveLength(0);

  await page.keyboard.press('Escape');
  await expect(page.getByTestId('attachment-preview')).toHaveCount(0);
  expect(
    await page.evaluate(() => document.activeElement?.getAttribute('data-attachment-focus')),
    'closing the preview returns focus to the same attachment',
  ).toBe(`open-${PDF}`);
});

test('attachments: cancelling a preview leaves a separately saved file alone', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await attachmentButton(page, 'save', PDF).click();
  await expect(page.getByText('Saved invoice.pdf')).toBeVisible();
  const before = await page.evaluate(() => window.__siftFixture!.control.savedPaths());
  expect(before).toEqual(['invoice.pdf']);
  const saved = await page.evaluate(() => window.__siftFixture!.control.savedAs());
  expect(saved[0].attachmentId, 'Save As targets the same account-qualified part').toBe(PDF);

  await attachmentButton(page, 'open', PDF).focus();
  await page.keyboard.press(' ');
  await expect(page.getByTestId('attachment-preview')).toBeVisible();
  await page.getByTestId('attachment-preview-close').click();
  await expect(page.getByTestId('attachment-preview')).toHaveCount(0);

  expect(await page.evaluate(() => window.__siftFixture!.control.savedPaths())).toEqual(before);
});

test('attachments: preview handles images and refuses unknown types', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();
  await page.getByTestId('attachment-show-all').click();

  await attachmentButton(page, 'open', PNG).focus();
  await page.keyboard.press(' ');
  await expect(page.getByTestId('attachment-preview-image')).toHaveAttribute(
    'src',
    `sift-att://acc-a/blue-00-m1/${PNG}`,
  );
  await page.keyboard.press('Escape');

  await attachmentButton(page, 'open', TXT).focus();
  await page.keyboard.press(' ');
  await expect(page.getByText('No preview for this file type.')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('attachment-preview')).toHaveCount(0);
});

test('attachments: saving keeps focus on the attachment that was activated', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  const save = attachmentButton(page, 'save', TXT);
  await save.focus();
  await page.keyboard.press(' ');
  await expect(page.getByText('Saved notes.txt')).toBeVisible();

  expect(await page.evaluate(() => document.activeElement?.getAttribute('data-attachment-focus'))).toBe(
    `save-${TXT}`,
  );
});
