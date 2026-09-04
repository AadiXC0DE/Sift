import { describe, expect, it } from 'vitest';
import { decodeRfc2047 } from './rfc2047';

describe('decodeRfc2047', () => {
  it('passes through plain text', () => {
    expect(decodeRfc2047('Hello')).toBe('Hello');
  });
  it('decodes Q-encoded UTF-8 with an em dash', () => {
    expect(decodeRfc2047('=?UTF-8?Q?Referral_Request_=E2=80=93_Intern?=')).toBe('Referral Request – Intern');
  });
  it('joins adjacent encoded words', () => {
    expect(decodeRfc2047('=?UTF-8?Q?Hello_?= =?UTF-8?Q?World?=')).toBe('Hello World');
  });
});
