import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

declare const process: { env: Record<string, string | undefined> };
const host = process.env.TAURI_DEV_HOST;
const isTauri = !!process.env.TAURI_ENV_PLATFORM;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    target: isTauri ? 'chrome105' : 'esnext',
    minify: !isTauri ? 'esbuild' : true,
    sourcemap: !!isTauri,
    chunkSizeWarningLimit: 300,
    // Emitted as dist/.vite/manifest.json; scripts/check-bundle.sh walks it to
    // find every statically imported chunk instead of guessing at "main".
    manifest: true,
  },
  test: undefined as unknown as Record<string, unknown>,
});
