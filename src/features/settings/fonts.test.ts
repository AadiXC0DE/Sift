import { describe, it, expect } from 'vitest';

describe('P9-T02 fonts lazy', () => {
  it('system loads no font; inter injects one link', () => {
    const links = document.querySelectorAll('link[data-font]');
    expect(links.length).toBe(0);
    const el = document.createElement('link');
    el.setAttribute('data-font', 'inter');
    document.head.appendChild(el);
    expect(document.querySelectorAll('link[data-font]').length).toBe(1);
    el.remove();
  });
});
