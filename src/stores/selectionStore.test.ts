import { describe, it, expect, beforeEach } from 'vitest';
import { useSelection } from './selectionStore';

describe('P4-T05 selection', () => {
  beforeEach(() => {
    useSelection.setState({ focusedKey: null, selectedIds: new Set(), anchorKey: null });
  });
  it('move stays at the last row; shift-extend selects a contiguous range; toggle; Esc clears but keeps focus', () => {
    const keys = ['id0', 'id1', 'id2', 'id3', 'id4', 'id5'];
    const s = useSelection.getState();
    s.setFocus('id5');
    s.move(1, keys);
    expect(useSelection.getState().focusedKey).toBe('id5');
    useSelection.setState({ focusedKey: 'id0', anchorKey: 'id0', selectedIds: new Set() });
    useSelection.getState().extendTo('id1', keys);
    useSelection.getState().extendTo('id2', keys);
    expect([...useSelection.getState().selectedIds].sort()).toEqual(['id0', 'id1', 'id2']);
    expect(useSelection.getState().focusedKey).toBe('id2');
    useSelection.getState().toggle('id1');
    expect(useSelection.getState().selectedIds.has('id1')).toBe(false);
    useSelection.getState().clearKeepFocus();
    expect(useSelection.getState().selectedIds.size).toBe(0);
    expect(useSelection.getState().anchorKey).toBeNull();
    expect(useSelection.getState().focusedKey).toBe('id2');
  });
  it('selectAll marks the loaded filtered window', () => {
    useSelection.getState().selectAll(['a:1', 'b:2', 'a:3']);
    expect(useSelection.getState().selectedIds.size).toBe(3);
    expect(useSelection.getState().anchorKey).toBe('a:1');
  });
});
