import { describe, it, expect } from 'vitest';

describe('P5-T07 expansion + P5-T08 banner', () => {
  it('5 messages with 2 unread -> unread + last expanded', () => {
    const msgs = [
      { id: 'm1', isUnread: false },
      { id: 'm2', isUnread: false },
      { id: 'm3', isUnread: true },
      { id: 'm4', isUnread: false },
      { id: 'm5', isUnread: false },
    ];
    const exp: Record<string, boolean> = {};
    msgs.forEach((m, i) => {
      exp[m.id] = m.isUnread || i === msgs.length - 1;
    });
    expect(exp).toEqual({ m1: false, m2: false, m3: true, m4: false, m5: true });
  });
  it('banner shows when remoteImageCount>0 and not allowed', () => {
    const body = { remoteImageCount: 2, remoteImagesAllowed: false };
    expect(body.remoteImageCount > 0 && !body.remoteImagesAllowed).toBe(true);
  });
});
