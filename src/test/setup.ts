import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

// Mock @tauri-apps/api invoke/listen
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

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
