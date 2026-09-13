// One row-height token shared by the row renderer, the virtualizer and the
// density-change restore path. Keep the three densities in sync here.
export const ROW_HEIGHTS = {
  compact: 32,
  default: 40,
  comfortable: 48,
} as const;

export type DensityName = keyof typeof ROW_HEIGHTS;

export const DEFAULT_ROW_HEIGHT = ROW_HEIGHTS.default;

export function rowHeightForDensity(density: string | null | undefined): number {
  if (density && density in ROW_HEIGHTS) return ROW_HEIGHTS[density as DensityName];
  return DEFAULT_ROW_HEIGHT;
}

/**
 * Scroll offset that keeps the same row at the top of the viewport after the
 * density (and therefore the row height) changes. The offset inside the row is
 * scaled so the reading position barely moves.
 */
export function densityScrollTop(
  topIndex: number,
  offsetWithin: number,
  prevHeight: number,
  nextHeight: number,
): number {
  const scaled = prevHeight > 0 ? (offsetWithin * nextHeight) / prevHeight : 0;
  const within = Math.min(Math.max(scaled, 0), Math.max(nextHeight - 1, 0));
  return Math.max(0, topIndex * nextHeight + within);
}
