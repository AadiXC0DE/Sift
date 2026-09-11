import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: 'e2e',
  timeout: 30000,
  use: { baseURL: 'http://localhost:4321' },
  webServer: { command: 'pnpm dev --port 4321', port: 4321, reuseExistingServer: true },
});
