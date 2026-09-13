/**
 * Shared Playwright harness.
 *
 * Every spec imports `test`/`expect` from here instead of `@playwright/test`
 * so that three failure classes cannot be silently ignored:
 *
 * 1. unexpected `console.error` output,
 * 2. uncaught page errors and unhandled promise rejections,
 * 3. fixture commands the app called that the mock does not implement.
 *
 * A test that only asserts `#root` exists cannot pass: the guards would not
 * catch that, but `gotoApp` waits for a rendered, seeded inbox.
 */
import { test as base, expect, type Locator, type Page } from '@playwright/test';
import { E2E_ENTRY } from './entry';

/** Dev-mode noise that is not an application defect. */
const ALLOWED_CONSOLE: RegExp[] = [/Download the React DevTools/, /\[vite\] connect/];

export interface AppFixture {
  gotoApp: (opts?: { scenario?: 'default' | 'empty' }) => Promise<void>;
  rows: () => Locator;
  row: (threadId: string) => Locator;
  rowIds: () => Promise<string[]>;
  /** Waits for seeded data, real system fonts and a settled animation frame. */
  ready: () => Promise<void>;
}

async function waitForSeededInbox(page: Page): Promise<void> {
  await page.locator('[data-testid^="row-"]').first().waitFor({ state: 'visible', timeout: 15_000 });
}

export const test = base.extend<{ guards: void; app: AppFixture }>({
  guards: [
    async ({ page }, provide) => {
      const errors: string[] = [];
      await page.addInitScript(() => {
        window.addEventListener('unhandledrejection', (e) => {
          const reason = (e as PromiseRejectionEvent).reason as { message?: string } | undefined;
          console.error(`[unhandledrejection] ${reason?.message ?? String(reason)}`);
        });
      });
      page.on('console', (msg) => {
        if (msg.type() !== 'error') return;
        const text = msg.text();
        const url = msg.location()?.url ?? '';
        // `sift-att://` is a Tauri-registered custom scheme. A plain browser
        // cannot resolve it, so inline-image thumbnails always log a resource
        // load error here (Chromium: ERR_UNKNOWN_URL_SCHEME, WebKit:
        // "unsupported URL"); it is an environment limitation, not an app error.
        if (url.startsWith('sift-att://')) return;
        if (ALLOWED_CONSOLE.some((re) => re.test(text))) return;
        errors.push(`console.error: ${text}`);
      });
      page.on('pageerror', (err) => errors.push(`pageerror: ${err.message}`));

      await provide();

      const unimplemented = await page
        .evaluate(() => window.__siftFixture?.control.unimplemented() ?? [])
        .catch(() => [] as string[]);
      expect(unimplemented, 'the app invoked a command the fixture backend does not implement').toEqual([]);
      expect(errors, 'unexpected browser errors').toEqual([]);
    },
    { auto: true },
  ],

  app: async ({ page }, provide) => {
    await provide({
      gotoApp: async (opts) => {
        const query = opts?.scenario === 'empty' ? '?scenario=empty' : '';
        await page.goto(`${E2E_ENTRY}${query}`);
        await expect(page.locator('#root')).toBeAttached();
        if (opts?.scenario !== 'empty') await waitForSeededInbox(page);
      },
      rows: () => page.locator('[data-testid^="row-"]'),
      row: (threadId) => page.locator(`[data-testid="row-${threadId}"]`),
      rowIds: async () =>
        page.$$eval('[data-testid^="row-"]', (els) =>
          els.map((el) => (el.getAttribute('data-testid') ?? '').replace(/^row-/, '')),
        ),
      ready: async () => {
        await page.evaluate(async () => {
          await document.fonts.ready;
          await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
        });
      },
    });
  },
});

export { expect };
