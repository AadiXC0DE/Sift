import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// Mock @tauri-apps/api invoke/listen
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: () => false }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

/*
 * `@tauri-apps/plugin-updater` extends `Resource` from the mocked core module
 * above, so importing the real package under jsdom throws while the module is
 * being evaluated — before a test can mock anything. This default keeps the
 * import harmless, and any call that is not staged by the test fails loudly
 * instead of quietly answering "no update".
 */
vi.mock('@tauri-apps/plugin-updater', () => ({
  check: vi.fn(async () => {
    throw new Error('the updater plugin is not available in this test');
  }),
  Update: class {},
}));

class RO {
  observe() {}
  unobserve() {}
  disconnect() {}
}
(globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = RO;
class IO {
  constructor(private cb: unknown) {}
  observe() {}
  unobserve() {}
  disconnect() {}
}
(globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = IO;

if (!(Element.prototype as unknown as { scrollIntoView?: unknown }).scrollIntoView) {
  (Element.prototype as unknown as { scrollIntoView: () => void }).scrollIntoView = () => {};
}

/*
 * jsdom does not implement `matchMedia`, and `applyTheme` reads it on every
 * settings change. Never matching keeps the document in light mode, which is
 * what the tests that assert a theme expect.
 */
if (typeof window.matchMedia !== 'function') {
  // The shape is jsdom's gap, not the app's: only `matches` and the listener
  // pair are ever touched, and a fuller MediaQueryList cannot be produced here.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}
