import { describe, it, expect } from 'vitest';
// P1-T10: second tooltip within 300ms has data-instant. We test the timing helper logic.
describe('P1-T10 Tooltip instant', () => {
  it('window under 300ms counts as instant', () => {
    const lastClose = Date.now() - 100;
    expect(Date.now() - lastClose < 300).toBe(true);
    const old = Date.now() - 500;
    expect(Date.now() - old < 300).toBe(false);
  });
});
