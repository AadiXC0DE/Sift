import type { FixtureControl } from './backend';
import type { UpdaterControl, UpdaterScenario } from './updater-scenario';

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
    /** The scenario the updater stub answers from (P11.1); set before load. */
    __SIFT_UPDATER__?: UpdaterScenario;
    /** What the app asked the updater stub to do, for assertions. */
    __siftUpdater?: UpdaterControl;
    __sift_xss?: number;
  }
}

export {};
