import path from 'node:path';
import { createRequire } from 'node:module';
import { expect, test } from './fixture/test';

const axePath = createRequire(import.meta.url).resolve('axe-core/axe.min.js');

/**
 * P9-T12 / P9.5: run the real axe-core engine (already a dev dependency) over
 * the seeded shell instead of asserting that `#root` exists.
 */
test('a11y: the seeded inbox has no serious or critical axe-core violations', async ({ page, app }) => {
  // Known P9.5 defects this gate records (remove `test.fail()` once fixed):
  //  - aria-allowed-attr + aria-required-children: AccountSwitcher puts
  //    aria-selected on plain buttons inside a role=tablist.
  //  - color-contrast: account avatars, the sidebar count, row subjects/dates.
  //  - scrollable-region-focusable: the thread listbox has no tab stop.
  test.fail();
  await app.gotoApp();
  await app.ready();
  await page.addScriptTag({ path: path.resolve(axePath) });

  const violations = await page.evaluate(async () => {
    interface AxeNode {
      target: string[];
    }
    interface AxeViolation {
      id: string;
      impact: string | null;
      help: string;
      nodes: AxeNode[];
    }
    const axe = (
      window as unknown as {
        axe: { run: (ctx: unknown, opts: unknown) => Promise<{ violations: AxeViolation[] }> };
      }
    ).axe;
    const result = await axe.run(document, {
      runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa'] },
    });
    return result.violations
      .filter((v) => v.impact === 'serious' || v.impact === 'critical')
      .map((v) => ({
        id: v.id,
        impact: v.impact,
        help: v.help,
        targets: v.nodes.map((n) => n.target.join(' ')),
      }));
  });

  expect(violations, `axe serious/critical: ${JSON.stringify(violations, null, 2)}`).toEqual([]);
});

test('a11y: the list is a named listbox with options and the reader exposes a heading', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  const list = page.getByRole('listbox', { name: 'Inbox' });
  await expect(list).toBeVisible();
  await expect(list.getByRole('option').first()).toBeVisible();
  await expect(list.getByRole('option', { selected: false }).first()).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Blue run 00');
  await expect(page.getByRole('textbox', { name: 'Search' })).toBeVisible();
});
