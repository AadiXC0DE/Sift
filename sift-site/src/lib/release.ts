/**
 * The one source of truth for the release version, the download URL and the
 * download size that this site displays.
 *
 * Nothing else in `src/` may hardcode a version tag, a DMG URL or a size: every
 * page renders `releaseView(await releaseState())`, so the three can never
 * disagree. The values come from the GitHub release API at build time, and a
 * size is only ever a byte count that API reported for the DMG asset of a
 * published release.
 *
 * When the API answers with no downloadable build, or does not answer at all,
 * the page shows no version and no size and points at the releases page
 * instead. Fabricating a version, a size or an available update is never an
 * option here: a "Download" button that leads to a 404 is worse than one that
 * says there is nothing to download yet.
 */

/** The human-facing releases page. Always valid, whatever the API says. */
export const RELEASES_PAGE = 'https://github.com/AadiXC0DE/Sift/releases';

/** Latest published (non-draft, non-prerelease) release; 404 when there is none. */
const RELEASES_API = 'https://api.github.com/repos/AadiXC0DE/Sift/releases/latest';

/** Asset the release pipeline uploads next to the DMG (`docs/release.md`). */
const SUMS_ASSET = 'SHA256SUMS';

export interface GitHubAsset {
  name: string;
  browser_download_url: string;
  size: number;
}

export interface GitHubReleasePayload {
  tag_name?: string;
  assets?: GitHubAsset[];
}

/** What the site knows about the current download. */
export type ReleaseState =
  /** A published release whose DMG asset a download button can point at. */
  | { kind: 'published'; version: string; url: string; sizeBytes: number; sha: string | null }
  /** The API answered: there is no downloadable macOS build, so no numbers. */
  | { kind: 'empty' }
  /** The API did not answer: we know nothing, so we claim nothing. */
  | { kind: 'unknown' };

/** Everything a page is allowed to render about the release, resolved once. */
export interface ReleaseView {
  /** True only when a published DMG backs the download button. */
  published: boolean;
  /** Published tag, or null when the API did not give us one. */
  version: string | null;
  /** Measured DMG size, or null when there is no published DMG to measure. */
  size: string | null;
  /** The same size, ready for a big-number slot: `{ value: '9.8', unit: 'MB' }`. */
  sizeParts: { value: string; unit: string } | null;
  /** Raw byte count behind `size`, for bar widths and structured data. */
  sizeBytes: number | null;
  /** Where the download button points: the asset, or the releases page. */
  href: string;
  /** What the download button says, given where it points. */
  label: string;
  /** SHA-256 of the DMG from the release's `SHA256SUMS` asset, or null. */
  sha: string | null;
  /** Version and size, in the order a download page reads them. */
  facts: string[];
  /** One honest sentence about why numbers are, or are not, on the page. */
  status: string;
}

interface FetchResponse {
  ok: boolean;
  status: number;
  text(): Promise<string>;
}

type FetchLike = (
  url: string,
  init?: { headers?: Record<string, string>; signal?: unknown },
) => Promise<FetchResponse>;

/** `fetch` exists in every supported runtime (Node 18.17+, Vercel, CI); check, don't assume. */
function requireFetch(): FetchLike {
  const candidate: unknown = Reflect.get(globalThis, 'fetch');
  if (typeof candidate !== 'function') throw new Error('this runtime has no global fetch');
  const fetchLike = candidate as FetchLike; // narrowed by the typeof check above
  return fetchLike;
}

/** A bounded request: a build must never hang on an unresponsive API. */
function timeoutSignal(ms: number): unknown {
  const AbortSignalCtor: unknown = Reflect.get(globalThis, 'AbortSignal');
  if (typeof AbortSignalCtor !== 'function') return undefined;
  const timeout: unknown = Reflect.get(AbortSignalCtor, 'timeout');
  if (typeof timeout !== 'function') return undefined;
  return (timeout as (ms: number) => unknown).call(AbortSignalCtor, ms);
}

/** Bytes split into the two pieces every size slot on the site renders. */
function sizeParts(bytes: number): { value: string; unit: string } {
  const mib = bytes / (1024 * 1024);
  return mib >= 1
    ? { value: mib.toFixed(1), unit: 'MB' }
    : { value: String(Math.round(bytes / 1024)), unit: 'KB' };
}

/**
 * Parse a GitHub release payload plus its `SHA256SUMS` text into the facts the
 * site is allowed to show. Throws rather than returning a half answer: a
 * release without a usable DMG asset must never produce a download button.
 */
export function parseRelease(
  payload: GitHubReleasePayload,
  sums: string,
): ReleaseState & { kind: 'published' } {
  const version = payload.tag_name;
  if (!version) throw new Error('release has no tag_name');
  const dmg = (payload.assets ?? []).find((a) => a.name.toLowerCase().endsWith('.dmg'));
  if (!dmg) throw new Error('no DMG asset (never ship a dead button)');
  if (!dmg.browser_download_url.includes('/releases/download/')) {
    throw new Error(`DMG url is not a release asset: ${dmg.browser_download_url}`);
  }
  if (!Number.isFinite(dmg.size) || dmg.size <= 0) throw new Error(`DMG asset has no size: ${dmg.name}`);
  const line = sums.split('\n').find((l) => l.includes(dmg.name));
  const digest = (line ?? '').trim().split(/\s+/)[0] ?? '';
  return {
    kind: 'published',
    version,
    url: dmg.browser_download_url,
    sizeBytes: dmg.size,
    sha: /^[0-9a-f]{64}$/i.test(digest) ? digest.toLowerCase() : null,
  };
}

async function readSums(
  payload: GitHubReleasePayload,
  doFetch: FetchLike,
  timeoutMs: number,
): Promise<string> {
  const asset = (payload.assets ?? []).find((a) => a.name === SUMS_ASSET);
  if (!asset) return '';
  try {
    const res = await doFetch(asset.browser_download_url, { signal: timeoutSignal(timeoutMs) });
    return res.ok ? await res.text() : '';
  } catch {
    // A missing checksum shortens the page; it never invents one.
    return '';
  }
}

/**
 * `{ authorization }` when a build token is configured, `{}` otherwise.
 *
 * An optional `SIFT_RELEASE_TOKEN` (set in the Vercel project's environment)
 * lifts GitHub's 60-per-hour per-IP budget for unauthenticated API calls, which
 * build runners share. The read must stay a static `import.meta.env.NAME`
 * member expression: Vite's dev module runner rejects dynamic access outright.
 */
function releaseTokenHeader(): Record<string, string> {
  const token: unknown = import.meta.env.SIFT_RELEASE_TOKEN;
  return typeof token === 'string' && token.length > 0 ? { authorization: `Bearer ${token}` } : {};
}

/**
 * Ask the release API what the current download is. Never throws and never
 * guesses: every failure path returns a state that advertises nothing.
 */
export async function fetchLatestRelease(
  doFetch: FetchLike = requireFetch(),
  timeoutMs = 5000,
): Promise<ReleaseState> {
  let res: FetchResponse;
  try {
    res = await doFetch(RELEASES_API, {
      headers: {
        accept: 'application/vnd.github+json',
        'user-agent': 'sift-site',
        ...releaseTokenHeader(),
      },
      signal: timeoutSignal(timeoutMs),
    });
  } catch {
    return { kind: 'unknown' };
  }
  // 404 is an answer, not a failure: the repository has no published release.
  if (res.status === 404) return { kind: 'empty' };
  if (!res.ok) return { kind: 'unknown' };
  let payload: GitHubReleasePayload;
  try {
    payload = JSON.parse(await res.text()) as GitHubReleasePayload;
  } catch {
    return { kind: 'unknown' };
  }
  const sums = await readSums(payload, doFetch, timeoutMs);
  try {
    return parseRelease(payload, sums);
  } catch {
    // A release exists but carries nothing we can offer (no DMG). Same page as
    // "nothing published": no version, no size, a link to the releases page.
    return { kind: 'empty' };
  }
}

let cached: Promise<ReleaseState> | null = null;

/**
 * The build's one answer, memoised so every page on the site renders the same
 * version, size and URL from a single request.
 */
export function releaseState(): Promise<ReleaseState> {
  cached ??= fetchLatestRelease();
  return cached;
}

/**
 * Turn the state into everything a page renders. `platform` and any other
 * caller-supplied facts (the bundle's minimum system version, and the like) are
 * appended to the version and the measured size; unmeasured parts are dropped
 * rather than filled in.
 */
export function releaseView(state: ReleaseState, ...extra: string[]): ReleaseView {
  if (state.kind !== 'published') {
    return {
      published: false,
      version: null,
      size: null,
      sizeParts: null,
      sizeBytes: null,
      href: RELEASES_PAGE,
      label: 'Get it on GitHub',
      sha: null,
      facts: [...extra],
      status:
        state.kind === 'empty'
          ? 'No macOS build is published yet, so there is no version or download size to show.'
          : 'The GitHub release API did not answer, so the current version and download size are unknown.',
    };
  }
  const parts = sizeParts(state.sizeBytes);
  return {
    published: true,
    version: state.version,
    size: `${parts.value} ${parts.unit}`,
    sizeParts: parts,
    sizeBytes: state.sizeBytes,
    href: state.url,
    label: 'Download for Mac',
    sha: state.sha,
    facts: [state.version, `${parts.value} ${parts.unit}`, ...extra],
    status: `The size above is the byte count GitHub reports for ${state.version}'s DMG asset.`,
  };
}
