import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: 'e2e',
  timeout: 30000,
  use: { baseURL: 'http://localhost:1420', trace: 'on-first-retry' },
  webServer: { command: 'pnpm dev:vite', port: 1420, reuseExistingServer: true },
});
