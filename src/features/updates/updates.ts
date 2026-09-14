/**
 * Update flow vocabulary (P11.1).
 *
 * The plugin does the work — `tauri-plugin-updater` fetches `latest.json`,
 * downloads the platform artifact and verifies its minisign signature against
 * `plugins.updater.pubkey` inside Rust, before any bytes reach the installer.
 * What happens here is the part the plugin cannot know: turning its errors into
 * something the user can act on, and keeping "we found an update" strictly
 * separate from "we installed one".
 *
 * The strings this module matches are the plugin's own `Display` impls, read
 * from the vendored crate (`tauri-plugin-updater-2.11.0/src/error.rs`,
 * `src/updater.rs`, `minisign-verify-0.2.5/src/lib.rs`) and from Tauri's ACL
 * rejection (`tauri-2.11.5/src/ipc/authority.rs`). Nothing here guesses at a
 * message it has not seen.
 */

/** Which step of the flow produced a failure; only used to word the message. */
export type UpdateStage = 'check' | 'download' | 'install' | 'relaunch';

export type UpdateFailureCode =
  /** The host has no network. Checked before any IPC call, so nothing was sent. */
  | 'offline'
  /** The updater has no endpoint configured, or the endpoint is not https. */
  | 'endpoint'
  /** The endpoint was reached (or not) and no usable response came back. */
  | 'network'
  /** A response arrived but is not a manifest this build understands. */
  | 'metadata'
  /** The manifest is fine but carries no artifact for this platform. */
  | 'platform'
  /** Signature verification rejected the artifact. Nothing was installed. */
  | 'signature'
  /** Tauri's ACL refused the call: the capability does not grant it. */
  | 'permission'
  | 'unknown';

export interface UpdateFailure {
  code: UpdateFailureCode;
  /** What the user can read and act on. */
  message: string;
  /** The untouched text from the plugin / IPC boundary, shown as the cause. */
  detail: string;
}

/** What the plugin told us about a newer release; the handle stays in the transport. */
export interface UpdateCandidate {
  version: string;
  /** RFC 3339, as the plugin formats it. */
  date?: string;
  notes?: string;
}

/** The last completed check, persisted so it survives a restart. */
export type LastCheckState = '' | 'up-to-date' | 'no-release' | 'available' | 'failed';

export interface LastCheck {
  at: number;
  state: LastCheckState;
  version: string;
  error: string;
}

export type CheckOutcome =
  | { kind: 'up-to-date' }
  /**
   * The endpoint answered, but published no release manifest. This is the
   * ordinary state of a project that has not cut a release yet (a 404 on
   * `latest.json`), so it is reported as a fact, never as an error.
   */
  | { kind: 'no-release' }
  | { kind: 'available'; candidate: UpdateCandidate }
  | { kind: 'failed'; failure: UpdateFailure };

/** The plugin's message when no configured endpoint produced a release. */
const NO_RELEASE = 'Could not fetch a valid release JSON from the remote';

/**
 * Whether an error means "there is nothing published", not "something broke".
 * A 404 on `latest.json`, an endpoint that never existed and a response the
 * updater loop walked past all arrive as this single message.
 */
export function isNoReleaseError(text: string): boolean {
  return text.includes(NO_RELEASE);
}

/** The plugin's own error text, whichever way the rejection crosses IPC. */
export function errorText(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  if (typeof error === 'object' && error !== null) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === 'string') return message;
    const inner = (error as { error?: unknown }).error;
    if (typeof inner === 'string') return inner;
    try {
      return JSON.stringify(error);
    } catch {
      return String(error);
    }
  }
  return String(error);
}

/**
 * minisign / signature-decoding failures. `minisign-verify` reports a bad
 * signature as "The signature verification failed", a key that does not match
 * as "The signature was created with a different key than the one provided",
 * and an unreadable key or signature body as "Invalid encoding in minisign
 * data" — which is what the placeholder pubkey in `tauri.conf.json` produces
 * today. `base64` and the plugin's `SignatureUtf8` variants are the same class
 * of failure: the artifact was not proven to be ours.
 */
const SIGNATURE_PATTERNS: RegExp[] = [
  /signature verification failed/i,
  /signature rejected/i,
  /Invalid encoding in minisign data/i,
  /signature was created with a different key/i,
  /signature algorithm is not supported/i,
  /Unexpected signature algorithm/i,
  /non-legacy mode signatures/i,
  /could not be decoded, please check if it is a valid base64 string/i,
  /Invalid symbol \d+, offset \d+/i,
  /Invalid last symbol/i,
  /Invalid padding/i,
  /6-bit remainder/i,
];

/** ACL refusals (`tauri-2.11.5/src/ipc/authority.rs`). */
const PERMISSION_PATTERNS: RegExp[] = [
  /not allowed\. Permissions associated with this command:/,
  /not allowed\. Plugin not found/,
  /not allowed\. Command not found/,
  /not allowed on window "/,
  /not allowed on origin \[/,
];

const ENDPOINT_PATTERNS: RegExp[] = [
  /Updater does not have any endpoints set/,
  /must use a secure protocol like `https`/,
];

const NETWORK_PATTERNS: RegExp[] = [
  /error sending request/i,
  /error trying to connect/i,
  /error connecting to/i,
  /connection refused/i,
  /connection reset/i,
  /dns error/i,
  /operation timed out/i,
  /request timed out/i,
  /timed out/i,
  /network is unreachable/i,
];

const PLATFORM_PATTERNS: RegExp[] = [
  /was not found in the response `platforms` object/,
  /were found in the response `platforms` object/,
  /Unsupported application architecture/,
  /Unsupported OS, expected one of/,
];

const METADATA_PATTERNS: RegExp[] = [
  /^missing field `/,
  /^invalid type:/,
  /^invalid value:/,
  /^unknown variant/,
  /^duplicate field/,
  /^expected .* at line \d+/,
  /^EOF while parsing/,
  /^trailing characters/,
  /error decoding response body/i,
  /latest\.json is not/i,
];

function messageFor(code: UpdateFailureCode, stage: UpdateStage): string {
  switch (code) {
    case 'offline':
      return 'Could not check for updates: this Mac is offline.';
    case 'endpoint':
      return 'This build has no usable update endpoint configured, so nothing can be checked.';
    case 'network':
      return stage === 'download'
        ? 'The update could not be downloaded. The download server could not be reached.'
        : 'Could not reach the update server. Check your connection and try again.';
    case 'metadata':
      return 'The update server answered with a release manifest this version of Sift cannot read, so nothing was downloaded.';
    case 'platform':
      return 'The published release has no build for this Mac, so there is nothing to install.';
    case 'signature':
      return stage === 'check'
        ? 'The update could not be verified, so nothing was installed.'
        : 'The update could not be verified, so it was not installed. Sift only installs builds signed with its own update key.';
    case 'permission':
      return 'This build does not permit the updater to run, so nothing was checked.';
    case 'unknown':
      return stage === 'install'
        ? 'The update was downloaded but could not be installed.'
        : stage === 'relaunch'
          ? 'Sift could not restart itself. Quit and open it again to finish the update.'
          : 'The update check failed.';
  }
}

/**
 * Turn anything the transport threw into something the panel can show.
 *
 * `online: false` is decided before the classifier runs, and the caller is
 * responsible for not calling the transport at all in that case.
 */
export function classifyUpdateError(
  error: unknown,
  options: { stage: UpdateStage; online?: boolean },
): UpdateFailure {
  const detail = errorText(error);
  const stage = options.stage;
  const some = (patterns: RegExp[]) => patterns.some((re) => re.test(detail));
  let code: UpdateFailureCode = 'unknown';
  if (options.online === false) code = 'offline';
  else if (some(PERMISSION_PATTERNS)) code = 'permission';
  else if (some(SIGNATURE_PATTERNS)) code = 'signature';
  else if (some(ENDPOINT_PATTERNS)) code = 'endpoint';
  else if (some(PLATFORM_PATTERNS)) code = 'platform';
  else if (METADATA_PATTERNS.some((re) => re.test(detail.trim()))) code = 'metadata';
  else if (some(NETWORK_PATTERNS)) code = 'network';
  return { code, message: messageFor(code, stage), detail };
}

/** The outcome of a failed check, with "nothing published" kept out of the failure bucket. */
export function checkOutcomeFromError(error: unknown, options: { online?: boolean } = {}): CheckOutcome {
  if (isNoReleaseError(errorText(error))) return { kind: 'no-release' };
  return { kind: 'failed', failure: classifyUpdateError(error, { stage: 'check', ...options }) };
}

/** The label the last-check record stores for an outcome. */
export function lastCheckStateOf(outcome: CheckOutcome): Exclude<LastCheckState, ''> {
  if (outcome.kind === 'failed') return 'failed';
  if (outcome.kind === 'available') return 'available';
  if (outcome.kind === 'no-release') return 'no-release';
  return 'up-to-date';
}

/**
 * Human summary of the persisted last check. A check that has never run says
 * so; it never claims to be current.
 */
export function describeLastCheck(last: LastCheck): string {
  if (last.at === 0 || last.state === '') return 'Never checked for updates.';
  switch (last.state) {
    case 'up-to-date':
      return 'Up to date.';
    case 'no-release':
      return 'No published release yet.';
    case 'available':
      return last.version ? `Version ${last.version} was available.` : 'An update was available.';
    case 'failed':
      return last.error || 'The last check failed.';
  }
  return 'Never checked for updates.';
}

/** Release date as the manifest reports it, or null when it carried none. */
export function formatReleaseDate(date: string | undefined): string | null {
  if (!date) return null;
  const parsed = Date.parse(date);
  if (Number.isNaN(parsed)) return null;
  return new Date(parsed).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  });
}

/** Percent complete, or null while the total is still unknown. */
export function downloadPercent(received: number, total: number | null): number | null {
  if (total === null || total <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((received / total) * 100)));
}
