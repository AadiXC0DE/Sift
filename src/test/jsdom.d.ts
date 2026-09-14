/**
 * Minimal ambient typing for `jsdom`, which ships no declarations of its own.
 *
 * `src/features/thread-view/find.test.ts` needs a second, isolated DOM realm to
 * run the mail-frame shim in (the shim is a string that ships into a sandboxed
 * frame, so running it is the only honest way to test it). Only the surface the
 * tests use is declared — no `any`, so a wrong call site still fails to compile.
 */
declare module 'jsdom' {
  export interface JSDOMOptions {
    runScripts?: 'dangerously' | 'outside-only';
    url?: string;
    pretendToBeVisual?: boolean;
  }

  export class JSDOM {
    constructor(html?: string, options?: JSDOMOptions);
    readonly window: Window & typeof globalThis;
    serialize(): string;
  }
}
