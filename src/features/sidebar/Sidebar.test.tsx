import { describe, it, expect } from 'vitest';

describe('P4-T13 nested labels', () => {
  it('Clients/Acme groups under Clients', () => {
    const labels = ['Clients/Acme', 'Clients/Globex', 'Receipts'];
    const roots: Record<string, string[]> = {};
    const top: string[] = [];
    for (const l of labels) {
      const parts = l.split('/');
      if (parts.length === 1) top.push(l);
      else (roots[parts[0]] ??= []).push(l);
    }
    expect(top).toEqual(['Receipts']);
    expect(roots['Clients']).toEqual(['Clients/Acme', 'Clients/Globex']);
  });
});
