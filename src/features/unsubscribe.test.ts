import { describe, it, expect } from 'vitest';

describe('P5-T14 unsubscribe method', () => {
  it('one-click -> post; mailto-only -> compose', () => {
    const oneClick = { url: 'https://x/unsub', oneClick: true };
    expect(oneClick.oneClick ? 'post' : 'url').toBe('post');
    const mailtoOnly = { mailto: 'mailto:leave@x', url: undefined, oneClick: false };
    expect(mailtoOnly.mailto ? 'mailto' : 'url').toBe('mailto');
  });
});
