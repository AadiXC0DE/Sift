import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

declare const process: { env: Record<string, string | undefined> };
const host = process.env.TAURI_DEV_HOST;
// The Tauri CLI sets this to 'darwin' | 'windows' | 'linux' for Tauri builds and
// leaves it unset for plain Vite builds (vitest, Playwright, `pnpm build`).
const tauriPlatform = process.env.TAURI_ENV_PLATFORM;
const isTauri = !!tauriPlatform;

// P9.5 — the rendering webview decides the syntax floor, not the machine running
// the build. macOS and Linux Tauri render with WebKit (WKWebView / WebKitGTK),
// which trails Chromium by years: the macOS 13 deployment floor ships Safari 16,
// so chrome105 output would use syntax that WebKit cannot even parse and the
// bundle would die on load. `safari16` is the honest WebKit floor. Windows Tauri
// renders with WebView2 (Chromium) and keeps chrome105; non-Tauri builds run on
// a current engine and stay esnext.
const buildTarget = !isTauri ? 'esnext' : tauriPlatform === 'windows' ? 'chrome105' : 'safari16';

export default defineConfig({
  // Different Tauri aliases must never invalidate the running app's modules.
  cacheDir: 'node_modules/.vite-app',
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
    target: buildTarget,
    minify: !isTauri ? 'esbuild' : true,
    sourcemap: !!isTauri,
    chunkSizeWarningLimit: 300,
    // Emitted as dist/.vite/manifest.json; scripts/check-bundle.sh walks it to
    // find every statically imported chunk instead of guessing at "main".
    manifest: true,
  },
  test: undefined as unknown as Record<string, unknown>,
});
