/**
 * The update flow's vocabulary (P11.1).
 *
 * Every error string asserted here was read out of the vendored crate sources
 * the app actually runs (`tauri-plugin-updater-2.11.0/src/error.rs`,
 * `minisign-verify-0.2.5/src/lib.rs`, `tauri-2.11.5/src/ipc/authority.rs`), so
 * a change to that wording is caught here instead of turning a signature
 * failure into "something went wrong" in front of the user.
 */
import { describe, expect, it } from 'vitest';
import {
  checkOutcomeFromError,
  classifyUpdateError,
  describeLastCheck,
  downloadPercent,
  errorText,
  formatReleaseDate,
  isNoReleaseError,
  lastCheckStateOf,
} from './updates';

describe('update failure classification', () => {
  it('reads the placeholder-key failure as a signature rejection', () => {
    // What `plugins.updater.pubkey`'s placeholder produces inside
    // `verify_signature`: PublicKey::decode cannot read the second line.
    const failure = classifyUpdateError('Invalid encoding in minisign data', { stage: 'download' });
    expect(failure.code).toBe('signature');
    expect(failure.message).toMatch(/could not be verified/);
    expect(failure.message).toMatch(/not installed/);
  });

  it('reads every minisign rejection as a signature failure', () => {
    const messages = [
      'The signature verification failed',
      'The signature was created with a different key than the one provided',
      'This signature algorithm is not supported by this implementation',
      'Unexpected signature algorithm',
      'Invalid symbol 33, offset 5.',
      'Encoded text cannot have a 6-bit remainder.',
      'The signature abc could not be decoded, please check if it is a valid base64 string.',
    ];
    for (const message of messages) {
      expect(classifyUpdateError(message, { stage: 'download' }).code, message).toBe('signature');
    }
  });

  it('reports a missing endpoint configuration as an endpoint problem', () => {
    expect(classifyUpdateError('Updater does not have any endpoints set.', { stage: 'check' }).code).toBe(
      'endpoint',
    );
    expect(
      classifyUpdateError('The configured updater endpoint must use a secure protocol like `https`.', {
        stage: 'check',
      }).code,
    ).toBe('endpoint');
  });

  it('reports a release without this platform as a platform problem', () => {
    const failure = classifyUpdateError(
      'the platform `darwin-aarch64` was not found in the response `platforms` object',
      { stage: 'check' },
    );
    expect(failure.code).toBe('platform');
    expect(failure.message).toMatch(/no build for this Mac/);
    expect(
      classifyUpdateError(
        'None of the fallback platforms `["darwin-aarch64"]` were found in the response `platforms` object',
        { stage: 'check' },
      ).code,
    ).toBe('platform');
    expect(
      classifyUpdateError('Unsupported application architecture, expected one of `x86`, `aarch64`.', {
        stage: 'check',
      }).code,
    ).toBe('platform');
  });

  it('reports an unreadable manifest as a metadata problem', () => {
    for (const message of [
      'missing field `platforms` at line 1 column 20',
      'invalid type: string "x", expected a version at line 1 column 12',
      'error decoding response body',
    ]) {
      expect(classifyUpdateError(message, { stage: 'check' }).code, message).toBe('metadata');
    }
  });

  it('reports an unreachable host as a network problem', () => {
    const failure = classifyUpdateError(
      'error sending request for url (https://github.com/AadiXC0DE/Sift/releases/latest/download/latest.json): ' +
        'error trying to connect: dns error: failed to lookup address information',
      { stage: 'check' },
    );
    expect(failure.code).toBe('network');
    expect(failure.message).toMatch(/Check your connection/);
  });

  it('reports a refused IPC call as a permission problem, not a silent no-op', () => {
    // Tauri's ACL rejection (`ipc/authority.rs`), which is what an unpermitted
    // updater call rejects with at the boundary.
    const failure = classifyUpdateError(
      'updater.check not allowed. Permissions associated with this command: updater:allow-check',
      { stage: 'check' },
    );
    expect(failure.code).toBe('permission');
    expect(failure.message).toMatch(/does not permit the updater/);
    expect(
      classifyUpdateError('updater.install not allowed. Plugin not found', { stage: 'install' }).code,
    ).toBe('permission');
  });

  it('decides offline before it looks at any message', () => {
    const failure = classifyUpdateError(
      'error sending request for url (https://example.test): connection refused',
      { stage: 'check', online: false },
    );
    expect(failure.code).toBe('offline');
    expect(failure.message).toMatch(/offline/);
  });

  it('keeps the plugin text verbatim so a report can name the real cause', () => {
    const failure = classifyUpdateError(new Error('The signature verification failed'), {
      stage: 'download',
    });
    expect(failure.detail).toBe('The signature verification failed');
  });
});

describe('check outcomes', () => {
  it('treats "no release manifest" as a fact, never as an error', () => {
    const error = 'Could not fetch a valid release JSON from the remote';
    expect(isNoReleaseError(error)).toBe(true);
    expect(checkOutcomeFromError(error)).toEqual({ kind: 'no-release' });
    expect(checkOutcomeFromError(new Error(error)).kind).toBe('no-release');
  });

  it('keeps a real failure a failure', () => {
    const outcome = checkOutcomeFromError(new Error('Could not reach the update server'));
    expect(outcome.kind).toBe('failed');
  });

  it('names the state each outcome is recorded as', () => {
    expect(lastCheckStateOf({ kind: 'up-to-date' })).toBe('up-to-date');
    expect(lastCheckStateOf({ kind: 'no-release' })).toBe('no-release');
    expect(
      lastCheckStateOf({
        kind: 'available',
        candidate: { version: '1.2.0' },
      }),
    ).toBe('available');
    expect(
      lastCheckStateOf({
        kind: 'failed',
        failure: classifyUpdateError('boom', { stage: 'check' }),
      }),
    ).toBe('failed');
  });
});

describe('last check reporting', () => {
  it('never claims a check that did not happen', () => {
    expect(describeLastCheck({ at: 0, state: '', version: '', error: '' })).toBe(
      'Never checked for updates.',
    );
    expect(describeLastCheck({ at: 1, state: '', version: '', error: '' })).toBe(
      'Never checked for updates.',
    );
  });

  it('reports each recorded state', () => {
    expect(describeLastCheck({ at: 1, state: 'up-to-date', version: '', error: '' })).toBe('Up to date.');
    expect(describeLastCheck({ at: 1, state: 'no-release', version: '', error: '' })).toBe(
      'No published release yet.',
    );
    expect(describeLastCheck({ at: 1, state: 'available', version: '1.4.0', error: '' })).toBe(
      'Version 1.4.0 was available.',
    );
    expect(describeLastCheck({ at: 1, state: 'failed', version: '', error: 'This Mac is offline.' })).toBe(
      'This Mac is offline.',
    );
  });
});

describe('presentation helpers', () => {
  it('formats a release date and refuses to invent one', () => {
    expect(formatReleaseDate(undefined)).toBeNull();
    expect(formatReleaseDate('not a date')).toBeNull();
    expect(formatReleaseDate('2026-09-13T00:00:00Z')).toMatch(/2026/);
  });

  it('reports progress only against a known total', () => {
    expect(downloadPercent(0, null)).toBeNull();
    expect(downloadPercent(50, 200)).toBe(25);
    expect(downloadPercent(300, 200)).toBe(100);
  });
});

describe('error text extraction', () => {
  it('reads strings, Errors and IPC objects alike', () => {
    expect(errorText('plain')).toBe('plain');
    expect(errorText(new Error('wrapped'))).toBe('wrapped');
    expect(errorText({ message: 'object message' })).toBe('object message');
    expect(errorText({ error: 'nested' })).toBe('nested');
  });
});
