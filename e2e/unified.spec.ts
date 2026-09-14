import { expect, test } from './fixture/test';
import { fixtureCalls, persistedThread, pressList, revealRow } from './fixture/helpers';

/**
 * UI-01 (P3.1): the account marker must read as one inset dash per row. It was
 * a full-height absolute stripe, so adjacent rows of the same account joined
 * into the reported long blue line.
 */
test('UI-01 unified list: no connected account stripe across adjacent same-account rows', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  const markers = page.locator('[data-testid="account-marker"]');
  expect(await markers.count()).toBeGreaterThan(8);

  const geometry = await page.evaluate(() => {
    const rows = [...document.querySelectorAll<HTMLElement>('[data-testid^="row-"]')];
    return rows.map((row) => {
      const box = row.getBoundingClientRect();
      const marker = row.querySelector<HTMLElement>('[data-testid="account-marker"]');
      const m = marker?.getBoundingClientRect();
      return {
        id: (row.dataset.testid ?? '').replace(/^row-/, ''),
        rowTop: box.top,
        rowBottom: box.bottom,
        rowHeight: box.height,
        markerTop: m?.top ?? null,
        markerBottom: m?.bottom ?? null,
        markerHeight: m?.height ?? null,
        markerWidth: m?.width ?? null,
        markerColor: marker ? getComputedStyle(marker).backgroundColor : null,
      };
    });
  });

  // The 30 newest unified rows all belong to the blue account.
  const blueRows = geometry.filter((g) => g.id.startsWith('blue-'));
  expect(blueRows.length).toBeGreaterThan(20);

  for (const row of geometry) {
    expect(row.rowHeight).toBe(40);
    if (row.markerHeight === null) continue;
    // An inset dash: shorter than the row and never touching either edge.
    expect(row.markerHeight).toBe(8);
    expect(row.markerWidth).toBe(2);
    expect(row.markerTop! - row.rowTop).toBeGreaterThanOrEqual(4);
    expect(row.rowBottom - row.markerBottom!).toBeGreaterThanOrEqual(4);
  }

  // Adjacent same-account markers must not join into a continuous segment.
  const ordered = geometry.filter((g) => g.markerTop !== null).sort((a, b) => a.markerTop! - b.markerTop!);
  for (let i = 1; i < ordered.length; i++) {
    const gap = ordered[i].markerTop! - ordered[i - 1].markerBottom!;
    expect(gap, `markers for ${ordered[i - 1].id} and ${ordered[i].id} touch`).toBeGreaterThan(0);
  }

  // Each account keeps its own accent colour.
  const blue = geometry.find((g) => g.id.startsWith('blue-'))!.markerColor;
  const rose = geometry.find((g) => g.id.startsWith('mixed-0'))?.markerColor;
  expect(blue).not.toBeNull();
  expect(rose).not.toBeNull();
  expect(rose).not.toBe(blue);

  await test.info().attach('unified-account-markers.png', {
    body: await page.locator('#root').screenshot(),
    contentType: 'image/png',
  });
});

/**
 * UI-04 (P3.5): with one selected row and a different focused row, a command
 * must act on the selection, never on whatever the cursor happens to sit on.
 */
test('UI-04 selection wins over focus: archive acts on the selected row, not the focused one', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await (await revealRow(page, 'mixed-01')).click();
  await pressList(page, 'x'); // select mixed-01 (focus stays on it)
  await pressList(page, 'j'); // move focus to the next row without selecting it

  await expect(page.getByText('1 selected')).toBeVisible();
  await pressList(page, 'e');

  await expect(app.row('mixed-01')).toHaveCount(0);

  const archived = await persistedThread(page, 'mixed-01');
  const focused = await persistedThread(page, 'mixed-02');
  expect(archived.labelIds).not.toContain('INBOX');
  expect(focused.labelIds).toContain('INBOX');
});

/**
 * UI-02 (P3.2): reversed account responses must not let the slower click win.
 * Account A's thread_get is delayed past account B's, so a row that only
 * remembered the previous detail would render A's subject last.
 */
test('UI-02 reversed account responses: the reader shows the last clicked, account-qualified thread', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  const { accountA, accountB } = await page.evaluate(() => window.__siftFixture!.ids);
  await page.evaluate(
    (ids) => {
      window.__siftFixture!.control.delayFor('thread_get', (args) =>
        args.accountId === ids.accountA ? 500 : 20,
      );
    },
    { accountA, accountB },
  );

  await app.row('blue-01').click(); // account A, answers last
  await (await revealRow(page, 'mixed-00')).click(); // account B, answers first
  await page.waitForTimeout(900);

  const heading = page.locator('h1');
  await expect(heading).toHaveText('Mixed 00');
  await expect(heading).not.toHaveText('Blue run 01');

  // Clicking back to A must show A: the reader is not stuck on the last winner.
  await (await revealRow(page, 'blue-01')).click();
  await expect(heading).toHaveText('Blue run 01', { timeout: 5000 });
});

test('unified scope merges accounts and narrowing to one account removes the other rows', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();
  expect(await app.row('blue-00').count()).toBeGreaterThan(0);
  await revealRow(page, 'mixed-00');

  await page
    .getByRole('tablist', { name: 'Accounts' })
    .getByTitle(/ada@example\.test/)
    .click();
  await expect(app.row('mixed-00')).toHaveCount(0);
  expect(await app.row('blue-00').count()).toBeGreaterThan(0);
  // One account in scope: the account marker carries no information.
  await expect(page.locator('[data-testid="account-marker"]')).toHaveCount(0);

  await page
    .getByRole('tablist', { name: 'Accounts' })
    .getByTitle(/All accounts/)
    .click();
  await revealRow(page, 'mixed-00');

  const queries = await fixtureCalls(page, 'threads_query');
  const accountIds = queries.map((q) => (q.query as { accountIds: string[] }).accountIds.join(','));
  expect(accountIds).toContain('acc-a');
  expect(accountIds[accountIds.length - 1]).toBe('acc-a,acc-b');
});
