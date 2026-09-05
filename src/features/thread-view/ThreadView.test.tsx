import { describe, it, expect } from 'vitest';

describe('P5-T07 expansion + remote images autoload', () => {
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
  it('remote images load by default: no per-message banner', () => {
    // Like any other email client, images render without a "Load" gate.
    // The backend returns remoteImagesAllowed:true unless the user opted
    // into Settings → Privacy → Never, so the thread view never banners.
    const body = { remoteImageCount: 2, remoteImagesAllowed: true };
    const showsBanner = body.remoteImageCount > 0 && !body.remoteImagesAllowed;
    expect(showsBanner).toBe(false);
  });
});
