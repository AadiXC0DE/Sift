import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { LICENSE_NAME, LICENSE_URL, REPO_URL, ISSUES_URL } from './project';

describe('project identity', () => {
  it('points the license link at a file that exists in the repository', () => {
    // The site calls Sift "open source under the MIT License" on every page. That
    // claim is only true while the LICENSE file the link resolves to is present
    // and actually grants MIT, so read the file the link names.
    const prefix = `${REPO_URL}/blob/main/`;
    expect(LICENSE_URL.startsWith(prefix)).toBe(true);
    const path = LICENSE_URL.slice(prefix.length);
    const text = readFileSync(new URL(`../../../${path}`, import.meta.url), 'utf8');
    expect(text).toContain(`${LICENSE_NAME} License`);
    expect(text).toContain('Permission is hereby granted, free of charge');
    expect(text).toMatch(/Copyright \(c\) \d{4} /);
  });

  it('addresses the repository and its issue tracker by one constant', () => {
    expect(REPO_URL).toBe('https://github.com/AadiXC0DE/Sift');
    expect(ISSUES_URL).toBe(`${REPO_URL}/issues`);
  });
});
