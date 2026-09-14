import { expect, test } from './fixture/test';
import { listScroller, revealRow } from './fixture/helpers';

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

/**
 * A message taller than any plausible default frame height must size its own
 * frame. When the reader stops hearing the size message the frame stays at its
 * initial height and the message is clipped behind an inner scrollbar - a
 * regression nothing else in this suite can see, because every other fixture
 * body is a few lines tall.
 */
test('reader: a long message grows its frame instead of clipping behind a scrollbar', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  // The long conversation is the oldest row in the fixture, so it is the last
  // one paged in: scroll to the bottom until the list has loaded far enough.
  const row = page.locator('[data-testid="row-long-0"]');
  const scroller = listScroller(page);
  for (let i = 0; i < 40 && (await row.count()) === 0; i++) {
    await scroller.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await page.waitForTimeout(150);
  }
  await expect(row).toHaveCount(1);
  await row.click();

  // Expand the tall message explicitly rather than relying on which messages
  // the reader opens by default.
  const header = page.getByTestId('msg-long-0-m196');
  await expect(header).toBeVisible();
  if ((await header.getAttribute('aria-expanded')) !== 'true') await header.click();

  /** The frame showing this message, found by content so a reordered reader cannot fool the test. */
  const frameShowing = async (marker: string) => {
    for (const handle of await page.$$('iframe[title="Email"]')) {
      const inner = await handle.contentFrame();
      if (!inner) continue;
      const text = await inner.evaluate(() => document.body.innerText).catch(() => '');
      if (text.includes(marker)) return handle;
    }
    return null;
  };

  await expect
    .poll(async () => (await frameShowing('Row 1 of a long message body.')) !== null, {
      message: 'the tall message must render in a frame',
    })
    .toBe(true);

  await expect
    .poll(
      async () => {
        const handle = await frameShowing('Row 1 of a long message body.');
        return handle ? Math.round((await handle.boundingBox())?.height ?? 0) : 0;
      },
      { message: 'the frame must grow past its initial height for a tall message' },
    )
    .toBeGreaterThan(400);

  // The frame is as tall as its content, so the message does not scroll inside
  // itself: everything is reachable by scrolling the reader.
  const handle = await frameShowing('Row 1 of a long message body.');
  // The frame is sandboxed without same-origin, so its document is only
  // reachable through Playwright's frame handle, never through contentDocument.
  const inner = await handle!.contentFrame();
  const scrolls = await inner!.evaluate(() => ({
    content: document.documentElement.scrollHeight,
    visible: document.documentElement.clientHeight,
  }));
  expect(scrolls.visible).toBeGreaterThanOrEqual(scrolls.content - 2);
});

// Exercise the production frame CSS and resize shim, including authored body
// styles and shrinking content (the root viewport must not become a height floor).
test('reader: authored mail colors survive and collapsed content removes inner scrolling', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();
  const frameElement = page.locator('iframe[title="Email"]').first();
  const handle = await frameElement.elementHandle();
  const frame = await handle!.contentFrame();
  await frame!.evaluate(() => {
    document.body.innerHTML = `<style>body { height:100% !important; background: rgb(244, 240, 232); color: rgb(35, 45, 55); font-family: Georgia; }</style><p>Authored newsletter</p><details open><summary>History</summary><div style="height:1800px">Long quoted message</div></details>`;
  });
  await expect.poll(async () => (await frameElement.boundingBox())!.height).toBeGreaterThan(1800);
  const colors = await frame!.evaluate(() => {
    const css = getComputedStyle(document.body);
    return { background: css.backgroundColor, color: css.color, font: css.fontFamily };
  });
  expect(colors).toEqual({ background: 'rgb(244, 240, 232)', color: 'rgb(35, 45, 55)', font: 'Georgia' });
  await frame!.locator('summary').click();
  await expect.poll(async () => (await frameElement.boundingBox())!.height).toBeLessThan(200);
  const geometry = await frame!.evaluate(() => ({
    content: document.documentElement.scrollHeight,
    viewport: document.documentElement.clientHeight,
  }));
  expect(geometry.content).toBeLessThanOrEqual(geometry.viewport + 2);
  await test.info().attach('reader-authored-styles.png', {
    body: await page.locator('#root').screenshot(),
    contentType: 'image/png',
  });
});
