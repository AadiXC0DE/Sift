import { describe, it, expect } from 'vitest';
import { basename, errorReason, physicalPointInRect, stagingFailureMessage } from './attachDrop';

const rect = { left: 100, top: 100, right: 700, bottom: 500 };

describe('P2.5 native drop hit test', () => {
  it('converts physical coordinates to CSS pixels with the scale factor', () => {
    expect(physicalPointInRect(rect, { x: 200, y: 200 }, 2)).toBe(true); // -> 100,100
    expect(physicalPointInRect(rect, { x: 1400, y: 1000 }, 2)).toBe(true); // -> 700,500
  });

  it('rejects a drop outside the composer', () => {
    expect(physicalPointInRect(rect, { x: 199, y: 200 }, 2)).toBe(false);
    expect(physicalPointInRect(rect, { x: 200, y: 1401 }, 2)).toBe(false);
    expect(physicalPointInRect(rect, { x: 0, y: 0 }, 2)).toBe(false);
  });

  it('falls back to scale 1 for a missing scale factor', () => {
    expect(physicalPointInRect(rect, { x: 200, y: 200 }, 0)).toBe(true);
  });
});

describe('P2.5 staging failure text', () => {
  it('names the rejected file and its reason', () => {
    expect(stagingFailureMessage(['/Users/me/Big.zip'], { message: 'Attachments exceed 25 MB' })).toBe(
      "Couldn't attach Big.zip: Attachments exceed 25 MB",
    );
  });

  it('does not repeat a filename the backend already included', () => {
    const message = stagingFailureMessage(['/Users/me/Big.zip'], {
      message: 'io: Big.zip: Is a directory (os error 21)',
    });
    expect(message).toBe("Couldn't attach: io: Big.zip: Is a directory (os error 21)");
  });

  it('summarizes long batches and accepts string errors', () => {
    const message = stagingFailureMessage(['/a/1', '/b/2', '/c/3', '/d/4'], 'disk full');
    expect(message).toContain('1, 2, 3 +1');
    expect(message).toContain('disk full');
  });

  it('extracts a reason from unknown error shapes', () => {
    expect(errorReason('boom')).toBe('boom');
    expect(errorReason(new Error('nope'))).toBe('nope');
    expect(errorReason(undefined)).toBe('unknown error');
  });

  it('takes the basename for both separators', () => {
    expect(basename('/a/b/c.txt')).toBe('c.txt');
    expect(basename('C:\\mail\\c.txt')).toBe('c.txt');
    expect(basename('c.txt')).toBe('c.txt');
  });
});
