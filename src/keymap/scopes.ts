import type { Scope } from './engine';

export interface ScopeInputs {
  /** Palette is the innermost overlay and owns its own keys. */
  paletteOpen: boolean;
  /** Any other overlay (compose, settings, help, add-account). */
  modalOpen: boolean;
  /** A conversation is open in the reader. */
  threadOpen: boolean;
}

/**
 * Active keymap scopes, innermost last. Overlays win: while a modal or palette
 * is open the list/thread scopes are absent, so a keystroke can never trigger a
 * hidden mail action.
 */
export function activeKeyScopes({ paletteOpen, modalOpen, threadOpen }: ScopeInputs): Scope[] {
  const scopes: Scope[] = ['global'];
  if (paletteOpen) {
    scopes.push('palette');
    return scopes;
  }
  if (modalOpen) return scopes;
  if (threadOpen) scopes.push('thread');
  scopes.push('list');
  return scopes;
}
