import { defineConfig } from '@playwright/test';
import { E2E_ENTRY, E2E_PORT } from './e2e/fixture/entry';

/**
 * E2E runs the real frontend against a deterministic fixture backend served by
 * the test-only Vite mode (`e2e/vite.e2e.config.ts`). Production entry points
 * are never aliased by `vite.config.ts`.
 *
 * Only browsers whose binaries are installed in this environment are declared
 * as projects; an absent browser must never be reported as executed.
 */
export default defineConfig({
  testDir: 'e2e',
  testMatch: /.*\.spec\.ts/,
  timeout: 45_000,
  expect: { timeout: 10_000 },
  fullyParallel: true,
  workers: 2,
  retries: 0,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
  use: {
    baseURL: `http://127.0.0.1:${E2E_PORT}`,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    colorScheme: 'light',
    locale: 'en-US',
    timezoneId: 'UTC',
  },
  // WebKit is the engine Tauri actually ships on macOS (WKWebView), so both
  // engines run. Firefox is not installed in this environment and is not
  // declared: an absent browser must never be reported as executed.
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
    { name: 'webkit', use: { browserName: 'webkit' } },
  ],
  webServer: {
    command: 'pnpm exec vite --config e2e/vite.e2e.config.ts',
    url: `http://127.0.0.1:${E2E_PORT}${E2E_ENTRY}`,
    reuseExistingServer: true,
    timeout: 60_000,
  },
});
