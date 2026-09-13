import { describe, it, expect } from 'vitest';
import { formatRowDate, relativeTime } from './dates';

describe('P4-T01 formatRowDate', () => {
  const now = new Date('2026-09-04T15:00:00').getTime();
  it('today -> HH:mm', () => {
    const t = new Date('2026-09-04T10:42:00').getTime();
    expect(formatRowDate(t, now)).toMatch(/10:42|10:42AM/);
  });
  it('3 days ago -> weekday', () => {
    const t = new Date('2026-09-01T10:00:00').getTime();
    expect(formatRowDate(t, now)).toMatch(/Tue/);
  });
  it('2 months ago -> d MMM', () => {
    const t = new Date('2026-07-12T10:00:00').getTime();
    expect(formatRowDate(t, now)).toMatch(/12 Jul/);
  });
  it('last year -> d MMM yyyy', () => {
    const t = new Date('2025-03-12T10:00:00').getTime();
    expect(formatRowDate(t, now)).toMatch(/2025/);
  });
});

describe('P3.6/P4.6 relativeTime', () => {
  const now = 1_000_000_000_000;
  it('only claims "just now" inside the minute', () => {
    expect(relativeTime(now - 5_000, now)).toBe('just now');
    expect(relativeTime(now - 90_000, now)).toBe('2 min ago');
  });
  it('scales to hours and days', () => {
    expect(relativeTime(now - 3 * 3_600_000, now)).toBe('3 h ago');
    expect(relativeTime(now - 2 * 86_400_000, now)).toBe('2 d ago');
  });
});
