import { defineConfig, type Project } from '@playwright/test';
import { E2E_ENTRY, E2E_PORT } from './e2e/fixture/entry';

/** `PW_PROJECTS=chromium,webkit` narrows a run; unset means every engine. */
const wantedProjects = process.env.PW_PROJECTS?.split(',').map((s) => s.trim());

const allProjects: Project[] = [
  { name: 'chromium', use: { browserName: 'chromium' } },
  { name: 'webkit', use: { browserName: 'webkit' } },
];

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
  // GitHub's macOS runners have 3 cores; the default (cpus/2) would give one
  // worker and run the whole suite serially. Tests wait on the app far more
  // than they compute, so oversubscribing the cores is the faster trade.
  workers: Number(process.env.PW_WORKERS ?? 2),
  retries: 0,
  // A stuck test must fail the job, not hold it for an hour: without a global
  // ceiling 168 serialized tests can each burn their 45s timeout.
  globalTimeout: process.env.CI ? 15 * 60_000 : 0,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
  use: {
    baseURL: `http://127.0.0.1:${E2E_PORT}`,
    // Tracing is captured for every test until it passes, which is pure
    // overhead on a slow runner. Opt in with PW_TRACE=1 when debugging.
    trace: process.env.PW_TRACE ? 'retain-on-failure' : 'off',
    screenshot: 'only-on-failure',
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    colorScheme: 'light',
    locale: 'en-US',
    timezoneId: 'UTC',
  },
  // WebKit is the engine Tauri actually ships on macOS (WKWebView), so both
  // engines run locally. CI selects one explicitly: Playwright's WebKit build
  // for macos-14 arm64 runners is frozen and every test there times out, so
  // PW_PROJECTS=chromium is set there and WebKit is verified on a real Mac.
  projects: allProjects.filter(
    (p) => !wantedProjects || (p.name !== undefined && wantedProjects.includes(p.name)),
  ),
  webServer: {
    // CI builds the fixture bundle in its own step (`pnpm e2e:bundle`) and
    // serves it statically here: dev mode transforms each module on demand and
    // 168 page loads pay for it. The build must not run inside this command,
    // or the 60s readiness timeout covers the build too.
    command: process.env.PW_PREVIEW
      ? 'pnpm exec vite preview --config e2e/vite.e2e.config.ts'
      : 'pnpm exec vite --config e2e/vite.e2e.config.ts',
    url: `http://127.0.0.1:${E2E_PORT}${E2E_ENTRY}`,
    reuseExistingServer: true,
    timeout: 60_000,
  },
});
