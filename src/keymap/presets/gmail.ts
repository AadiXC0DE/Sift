import type { Binding } from '../engine';
export const bindings: Binding[] = [
  { key: 'j', scope: 'list', action: 'focusNext' },
  { key: 'k', scope: 'list', action: 'focusPrev' },
  { key: 'y', scope: 'list', action: 'archive' },
  { key: '#', scope: 'list', action: 'trash' },
  { key: 's', scope: 'list', action: 'star' },
  { key: 'c', scope: 'global', action: 'compose' },
  { key: '/', scope: 'global', action: 'search' },
];
