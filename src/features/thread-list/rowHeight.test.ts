import { describe, expect, it } from 'vitest';
import { DEFAULT_ROW_HEIGHT, ROW_HEIGHTS, densityScrollTop, rowHeightForDensity } from './rowHeight';

describe('P3.5 shared row-height token', () => {
  it('exposes the three densities the renderer and virtualizer share', () => {
    expect(ROW_HEIGHTS).toEqual({ compact: 32, default: 40, comfortable: 48 });
    expect(rowHeightForDensity('compact')).toBe(32);
    expect(rowHeightForDensity('default')).toBe(40);
    expect(rowHeightForDensity('comfortable')).toBe(48);
    expect(rowHeightForDensity(undefined)).toBe(DEFAULT_ROW_HEIGHT);
    expect(rowHeightForDensity('nonsense')).toBe(DEFAULT_ROW_HEIGHT);
  });

  it('restores the top visible row and its offset across a density change', () => {
    // Row 12 at 40px rows, 10px into the row → 12*48 + 12 = 588 at 48px rows.
    expect(densityScrollTop(12, 10, 40, 48)).toBe(588);
    expect(densityScrollTop(0, 0, 40, 32)).toBe(0);
    // A within-row offset never overflows the new row height.
    expect(densityScrollTop(1, 39, 40, 32)).toBe(32 + 31);
  });
});
