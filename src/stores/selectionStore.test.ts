import { describe, it, expect, beforeEach } from 'vitest';
import { useSelection } from './selectionStore';

describe('P4-T05 selection', () => {
  beforeEach(() => {
    useSelection.setState({ focusedIndex: 0, selectedIds: new Set(), anchorIndex: null });
  });
  it('j at last row stays; shift+j twice selects 3; x toggles; Esc clears but keeps focus', () => {
    const s = useSelection.getState();
    s.setFocus(5);
    s.move(1, 6);
    expect(useSelection.getState().focusedIndex).toBe(5);
    useSelection.setState({ focusedIndex: 0, anchorIndex: 0, selectedIds: new Set() });
    const ids = (i: number) => `id${i}`;
    useSelection.getState().extendTo(1, ids);
    useSelection.getState().extendTo(2, ids);
    expect(useSelection.getState().selectedIds.size).toBe(3);
    useSelection.getState().toggle('id1');
    expect(useSelection.getState().selectedIds.has('id1')).toBe(false);
    useSelection.getState().clearKeepFocus();
    expect(useSelection.getState().selectedIds.size).toBe(0);
    expect(useSelection.getState().focusedIndex).toBe(2);
  });
});
