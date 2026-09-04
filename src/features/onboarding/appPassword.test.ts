import { describe, expect, it } from 'vitest';
import {
  appPasswordError,
  groupAppPassword,
  isValidAppPassword,
  looksLikeAppPassword,
  normalizeAppPassword,
} from './appPassword';

describe('P11-T16 app password normalize/validate', () => {
  it('normalizes "abcd efgh ijkl mnop", dashes, trailing space', () => {
    expect(normalizeAppPassword('abcd efgh ijkl mnop')).toBe('abcdefghijklmnop');
    expect(normalizeAppPassword('ABCD-EFGH-IJKL-MNOP')).toBe('abcdefghijklmnop');
    expect(normalizeAppPassword('abcdefghijklmnop ')).toBe('abcdefghijklmnop');
  });
  it('validates exactly 16 ASCII letters', () => {
    expect(isValidAppPassword('abcdefghijklmnop')).toBe(true);
    expect(isValidAppPassword('abcd efgh ijkl mnop')).toBe(true);
    expect(isValidAppPassword('abcdefghijklmno')).toBe(false);
    expect(isValidAppPassword('abcdefghijklmnopq')).toBe(false);
    expect(isValidAppPassword('abcd1234efgh5678')).toBe(false);
  });
  it('reports exact error copy', () => {
    expect(appPasswordError('abc')).toContain('16 letters');
    expect(appPasswordError('abcd1234efgh5678')).toContain('letters only');
  });
  it('groups display as 4x4', () => {
    expect(groupAppPassword('abcdefghijklmnop')).toBe('abcd efgh ijkl mnop');
  });
  it('matches clipboard assist shape only', () => {
    expect(looksLikeAppPassword('abcd efgh ijkl mnop')).toBe(true);
    expect(looksLikeAppPassword('ABCD EFGH IJKL MNOP')).toBe(true);
    expect(looksLikeAppPassword('hello world')).toBe(false);
    expect(looksLikeAppPassword('abcd efgh')).toBe(false);
  });
});
