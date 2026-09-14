/**
 * Test-only Vite mode. Not referenced by `vite.config.ts`, the Tauri config or
 * any production script: the aliases below only exist when this file is the
 * active config, so no seed/mock route can reach a shipped bundle.
 *
 * Usage:
 *   pnpm exec vite --config e2e/vite.e2e.config.ts          (Playwright webServer)
 *   pnpm exec vite build --config e2e/vite.e2e.config.ts    (static E2E bundle)
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const here = path.dirname(fileURLToPath(import.meta.url));
const fixtureDir = path.join(here, 'fixture');
const root = path.resolve(here, '..');

export default defineConfig({
  root,
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: [
      { find: /^@tauri-apps\/api\/core$/, replacement: path.join(fixtureDir, 'tauri-core.ts') },
      { find: /^@tauri-apps\/api\/event$/, replacement: path.join(fixtureDir, 'tauri-event.ts') },
      { find: /^@tauri-apps\/plugin-dialog$/, replacement: path.join(fixtureDir, 'tauri-dialog.ts') },
      { find: /^@tauri-apps\/plugin-updater$/, replacement: path.join(fixtureDir, 'tauri-updater.ts') },
    ],
  },
  server: {
    host: '127.0.0.1',
    port: 4399,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  // Serving the prebuilt bundle keeps the same URL as dev mode; CI uses it so
  // page loads do not pay for on-demand transforms 168 times over.
  preview: {
    host: '127.0.0.1',
    port: 4399,
    strictPort: true,
  },
  build: {
    // Playwright wipes `test-results/` at the start of every run, so the
    // fixture bundle lives beside it instead. Never next to the production
    // `dist/`, which `emptyOutDir` must not be able to wipe.
    outDir: '.e2e-build',
    emptyOutDir: true,
    rollupOptions: { input: path.join(fixtureDir, 'index.e2e.html') },
  },
});
