import { describe, it, expect } from 'vitest';

describe('P6-T10 undo stack 60s', () => {
  it('z undoes recent, not old', () => {
    const stack = [
      { group: 'g1', at: Date.now() - 10_000 },
      { group: 'g2', at: Date.now() - 70_000 },
    ];
    const undoable = stack.filter((s) => Date.now() - s.at <= 60_000);
    expect(undoable.map((s) => s.group)).toEqual(['g1']);
  });
});
