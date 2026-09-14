/**
 * A single ordered stack of the surfaces that can own Escape and the keyboard
 * (P9.5).
 *
 * At baseline every modal used its own boolean and its own window listener, so
 * Escape closed whichever surface happened to be checked first in `App`, and a
 * popover inside a dialog could be dismissed together with its parent. One
 * stack, pushed in mount/open order, makes "Escape closes the topmost surface"
 * a property of the code rather than of listener registration order.
 *
 * A `native` entry means the surface handles Escape itself (Base UI dialogs,
 * popovers and menus, the composer with its own Escape layers): the app-level
 * handler must let the key through to it. A `managed` entry is closed by the
 * app-level handler calling `close`.
 */

export interface Surface {
  id: number;
  /** `native`: the surface dismisses itself; `managed`: the stack closes it. */
  kind: 'native' | 'managed';
  close?: () => void;
  /** Optional tag so a self-dismissing surface can tell whether it is topmost. */
  owner?: string;
}

let nextId = 1;
const stack: Surface[] = [];

/** Push a surface; call the returned function to remove it. */
export function pushSurface(surface: {
  kind: Surface['kind'];
  close?: () => void;
  owner?: string;
}): () => void {
  const entry: Surface = { id: nextId++, ...surface };
  stack.push(entry);
  return () => {
    const index = stack.findIndex((s) => s.id === entry.id);
    if (index >= 0) stack.splice(index, 1);
  };
}

export function topSurface(): Surface | undefined {
  return stack[stack.length - 1];
}

/**
 * True while any surface owns the keyboard. List and reader shortcuts must not
 * fire then — "never send list shortcuts … while interacting with a menu".
 */
export function hasBlockingSurface(): boolean {
  return stack.length > 0;
}

/**
 * What the app-level Escape handler should do:
 * - `none`     — no surface is open; ordinary back-out handling applies.
 * - `pass`     — the topmost surface dismisses itself; let the key reach it.
 * - `handled`  — the topmost `managed` surface was closed by this call.
 *
 * A managed entry is removed from the stack *before* its `close` runs, so the
 * stack advances immediately: two Escapes in one frame cannot close the same
 * surface twice and never reach past the next one, whatever React's update
 * timing is. The surface's own cleanup removes the same id again (idempotent).
 */
export function handleEscape(): 'none' | 'pass' | 'handled' {
  const top = topSurface();
  if (!top) return 'none';
  if (top.kind === 'native') return 'pass';
  stack.pop();
  top.close?.();
  return 'handled';
}

/** Test-only reset. */
export function clearSurfaces(): void {
  stack.length = 0;
}
