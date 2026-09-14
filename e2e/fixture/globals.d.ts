import type { FixtureControl } from './backend';

declare global {
  // tsconfig targets ES2022, but every browser Playwright ships implements
  // `Promise.withResolvers` (Chrome 119+/WebKit 17.4+). Declared here so the
  // fixture can use the linear form without widening the app's lib target.
  interface PromiseConstructor {
    withResolvers<T>(): {
      promise: Promise<T>;
      resolve: (value: T | PromiseLike<T>) => void;
      reject: (reason?: unknown) => void;
    };
  }

  interface Window {
    __SIFT_E2E__?: boolean;
    /** The page's pinned clock (index.e2e.html); the single source of "now". */
    __SIFT_CLOCK__: { now: number };
    __siftFixture?: FixtureControl;
    __sift_xss?: number;
  }
}

export {};
