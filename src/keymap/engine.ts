import { useEffect, useRef } from 'react';

export type Scope = 'global' | 'list' | 'thread' | 'compose' | 'palette';
export interface Binding {
  key: string;
  scope: Scope;
  action: string;
}

const order: Scope[] = ['global', 'list', 'thread', 'compose', 'palette'];

export interface ResolvedAction {
  action: string;
  raw: string;
  scope: Scope;
}

/** Returns true when the handler owned the action (and stops the fan-out). */
export type ActionHandler = (resolved: ResolvedAction) => boolean;

export class KeymapEngine {
  bindings = new Map<string, Binding[]>();
  seq: string | null = null;
  seqTimer: ReturnType<typeof setTimeout> | undefined = undefined;
  lastTooltipAt = 0;
  onAction: (action: string, raw: string) => void = () => {};
  pendingHint: (s: string | null) => void = () => {};
  private handlers: ActionHandler[] = [];

  /** Feature scopes subscribe by scope; the engine already resolved precedence. */
  subscribe(handler: ActionHandler): () => void {
    this.handlers.push(handler);
    return () => {
      const i = this.handlers.indexOf(handler);
      if (i >= 0) this.handlers.splice(i, 1);
    };
  }

  private dispatch(action: string, raw: string, scope: Scope): void {
    for (const handler of [...this.handlers]) {
      if (handler({ action, raw, scope })) return;
    }
    this.onAction(action, raw);
  }

  register(bindings: Binding[]) {
    this.bindings.clear();
    for (const b of bindings) {
      const k = `${b.scope}:${b.key}`;
      if (!this.bindings.has(k)) this.bindings.set(k, []);
      this.bindings.get(k)!.push(b);
    }
  }

  scopesFor(active: Scope[]): Scope[] {
    // innermost last; lookup innermost outward
    return [...active].reverse();
  }

  handle(e: KeyboardEvent, active: Scope[]): boolean {
    const t = e.target as HTMLElement | null;
    const typing = !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
    if (e.key === 'Escape') {
      this.clearSeq();
      return false; // let Esc handlers run
    }
    // Never claim a key while an IME is composing; the candidate window owns it.
    if (e.isComposing || e.keyCode === 229) return false;
    if (typing) return false;
    const key = combo(e);
    // sequences: 'g' then 'i'
    if (this.seq) {
      const full = `${this.seq} ${key}`;
      const hit = this.lookup(full, active);
      this.clearSeq();
      if (hit) {
        e.preventDefault();
        this.dispatch(hit.action, full, hit.scope);
        return true;
      }
      return false;
    }
    // check if key is a sequence starter (any binding starts with "key ")
    if (this.isPrefix(key)) {
      e.preventDefault();
      this.seq = key;
      this.pendingHint(key);
      clearTimeout(this.seqTimer);
      this.seqTimer = setTimeout(() => this.clearSeq(), 800);
      return true;
    }
    const hit = this.lookup(key, active);
    if (hit) {
      e.preventDefault();
      this.dispatch(hit.action, key, hit.scope);
      return true;
    }
    return false;
  }

  lookup(key: string, active: Scope[]): Binding | undefined {
    for (const s of this.scopesFor(active)) {
      const arr = this.bindings.get(`${s}:${key}`);
      if (arr?.length) return arr[0];
    }
    // global fallback already in active if included; ensure global checked
    if (!active.includes('global')) {
      const arr = this.bindings.get(`global:${key}`);
      if (arr?.length) return arr[0];
    }
    void order;
    return undefined;
  }

  isPrefix(key: string): boolean {
    for (const k of this.bindings.keys()) {
      const bare = k.split(':')[1];
      if (bare.startsWith(`${key} `)) return true;
    }
    return false;
  }

  clearSeq() {
    this.seq = null;
    clearTimeout(this.seqTimer);
    this.seqTimer = undefined;
    this.pendingHint(null);
  }
}

export function combo(e: KeyboardEvent): string {
  const mods: string[] = [];
  if (e.metaKey) mods.push('⌘');
  if (e.ctrlKey) mods.push('⌃');
  if (e.altKey) mods.push('⌥');
  if (e.shiftKey) mods.push('⇧');
  let k = e.key;
  if (k === ' ') k = 'space';
  if (k === 'ArrowDown') k = '↓';
  if (k === 'ArrowUp') k = '↑';
  if (k === 'Enter') k = '↩';
  if (k === 'Backspace') k = '⌫';
  if (k.length === 1) k = k.toLowerCase();
  // shift+letter is just uppercase letter; keep ⇧ prefix only for special keys
  if (mods.join('') === '⇧' && /^[a-z]$/.test(k)) return `⇧${k}`;
  return mods.join('') + k;
}

export const engine = new KeymapEngine();

/**
 * Register handlers for the actions the engine resolved in `scope`. The engine
 * is the single keydown owner (see `App`); features only receive action names,
 * so remapping a binding never requires touching feature key handlers.
 */
export function useKeymap(scope: Scope, map: Record<string, () => void>) {
  const mapRef = useRef(map);
  mapRef.current = map;
  useEffect(
    () =>
      engine.subscribe(({ action, scope: resolved }) => {
        if (resolved !== scope) return false;
        const run = mapRef.current[action];
        if (!run) return false;
        run();
        return true;
      }),
    [scope],
  );
}
