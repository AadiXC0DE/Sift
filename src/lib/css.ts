/**
 * CSS feature detection for values that cannot be expressed as a CSS fallback
 * cascade (P9.5).
 *
 * `color-mix()` is unavailable on Safari 16.0/16.1, the WebKit floor this app
 * compiles to on macOS. A declaration such as
 * `background: rgb(...); background: color-mix(...)` does *not* fall back,
 * because a `var()` inside the value defers validation to computed-value time:
 * the later declaration always wins and an unsupported function resolves to the
 * property's initial value. Anything that tints an inline style therefore has to
 * decide at runtime and supply a real token instead.
 */
export const COLOR_MIX_SUPPORTED = (() => {
  try {
    // `CSS` exists in some engines without `supports` (jsdom), so both halves of
    // the guard matter: an unguarded call would throw while a module loads.
    return (
      typeof CSS !== 'undefined' &&
      typeof CSS.supports === 'function' &&
      CSS.supports('color', 'color-mix(in oklab, red, blue)')
    );
  } catch {
    return false;
  }
})();
