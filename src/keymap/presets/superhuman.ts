import type { Binding } from '../engine';
export const bindings: Binding[] = [
  { key: 'j', scope: 'list', action: 'focusNext' },
  { key: 'k', scope: 'list', action: 'focusPrev' },
  { key: 'e', scope: 'list', action: 'archive' },
  { key: '⌘k', scope: 'global', action: 'palette' },
  { key: 'h', scope: 'list', action: 'snooze' },
];
