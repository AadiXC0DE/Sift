import { describe, it, expect } from 'vitest';
import {
  RELEASES_PAGE,
  fetchLatestRelease,
  parseRelease,
  releaseView,
  type GitHubReleasePayload,
} from './release';

const dmgAsset = {
  name: 'Sift_9.9.9_universal.dmg',
  browser_download_url: 'https://github.com/AadiXC0DE/Sift/releases/download/v9.9.9/Sift_9.9.9_universal.dmg',
  size: 10_276_915,
};

const payload: GitHubReleasePayload = { tag_name: 'v9.9.9', assets: [dmgAsset] };

function response(status: number, body: unknown): { ok: boolean; status: number; text(): Promise<string> } {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (typeof body === 'string' ? body : JSON.stringify(body)),
  };
}

describe('parseRelease', () => {
  it('extracts version, immutable asset url, size and checksum', () => {
    const sums = `9f2c${'a'.repeat(60)}  Sift_9.9.9_universal.dmg\n`;
    const release = parseRelease(payload, sums);
    expect(release.kind).toBe('published');
    expect(release.version).toBe('v9.9.9');
    expect(release.url).toBe(dmgAsset.browser_download_url);
    expect(release.sizeBytes).toBe(10_276_915);
    expect(release.sha).toBe(`9f2c${'a'.repeat(60)}`);
  });

  it('never yields a download button without a DMG asset', () => {
    expect(() => parseRelease({ tag_name: 'v1', assets: [] }, '')).toThrow(/no DMG asset/);
    expect(() => parseRelease({ assets: [dmgAsset] }, '')).toThrow(/tag_name/);
  });

  it('refuses a DMG url that is not an immutable release asset', () => {
    const tarball = {
      ...dmgAsset,
      browser_download_url: 'https://github.com/AadiXC0DE/Sift/archive/v9.9.9.dmg',
    };
    expect(() => parseRelease({ tag_name: 'v9.9.9', assets: [tarball] }, '')).toThrow(/not a release asset/);
  });

  it('drops a checksum that SHA256SUMS does not actually contain', () => {
    const release = parseRelease(payload, 'unrelated sums line\n');
    expect(release.sha).toBeNull();
  });
});

describe('fetchLatestRelease', () => {
  it('reports empty — not a fabricated release — when the repository has published nothing', async () => {
    const state = await fetchLatestRelease(async () => response(404, { message: 'Not Found' }));
    expect(state).toEqual({ kind: 'empty' });
    expect(releaseView(state).version).toBeNull();
    expect(releaseView(state).size).toBeNull();
  });

  it('reports unknown when the endpoint does not answer, and advertises nothing', async () => {
    const state = await fetchLatestRelease(async () => {
      throw new Error('getaddrinfo ENOTFOUND api.github.com');
    });
    expect(state).toEqual({ kind: 'unknown' });
    const view = releaseView(state, 'macOS 13 or newer');
    expect(view.facts).toEqual(['macOS 13 or newer']);
    expect(view.status).toMatch(/did not answer/);
  });

  it('reports unknown on a server error or a body that is not JSON', async () => {
    expect(await fetchLatestRelease(async () => response(503, 'upstream unavailable'))).toEqual({
      kind: 'unknown',
    });
    expect(await fetchLatestRelease(async () => response(200, '<html>rate limited</html>'))).toEqual({
      kind: 'unknown',
    });
  });

  it('reports empty when a release exists but carries no DMG', async () => {
    const state = await fetchLatestRelease(async () =>
      response(200, {
        tag_name: 'v9.9.9',
        assets: [
          {
            name: 'Sift.app.tar.gz',
            size: 1,
            browser_download_url:
              'https://github.com/AadiXC0DE/Sift/releases/download/v9.9.9/Sift.app.tar.gz',
          },
        ],
      }),
    );
    expect(state).toEqual({ kind: 'empty' });
  });

  it('reads the size and checksum from a published release, and never invents one when SHA256SUMS is unreachable', async () => {
    const requested: string[] = [];
    const ok = await fetchLatestRelease(async (url) => {
      requested.push(url);
      if (url.endsWith('SHA256SUMS')) {
        return response(200, `${'b'.repeat(64)}  ${dmgAsset.name}\n`);
      }
      return response(200, {
        ...payload,
        assets: [
          ...payload.assets!,
          {
            name: 'SHA256SUMS',
            size: 90,
            browser_download_url: 'https://github.com/AadiXC0DE/Sift/releases/download/v9.9.9/SHA256SUMS',
          },
        ],
      });
    });
    expect(requested.some((u) => u.endsWith('SHA256SUMS'))).toBe(true);
    const view = releaseView(ok, 'macOS 13 or newer');
    expect(view.published).toBe(true);
    expect(view.size).toBe('9.8 MB');
    expect(view.label).toBe('Download for Mac');
    expect(view.href).toBe(dmgAsset.browser_download_url);
    expect(view.sha).toBe('b'.repeat(64));
    expect(view.facts).toEqual(['v9.9.9', '9.8 MB', 'macOS 13 or newer']);

    const noSums = await fetchLatestRelease(async (url) =>
      url.endsWith('SHA256SUMS') ? response(500, 'boom') : response(200, payload),
    );
    expect(releaseView(noSums).sha).toBeNull();
  });
});

describe('releaseView fallback', () => {
  it('points every unreleased state at the releases page instead of a dead asset', () => {
    for (const state of [{ kind: 'empty' as const }, { kind: 'unknown' as const }]) {
      const view = releaseView(state);
      expect(view.href).toBe(RELEASES_PAGE);
      expect(view.label).not.toMatch(/Download for Mac/);
      expect(view.published).toBe(false);
      expect(view.sha).toBeNull();
    }
  });
});
