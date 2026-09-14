/**
 * The update flow against a staged endpoint (P11.1).
 *
 * A real HTTP server on loopback serves a real `latest.json` and a real signed
 * archive, and the app's own store drives the decision through its transport
 * boundary. The verification itself is the release tooling's existing minisign
 * implementation (`scripts/lib/updater.ts`, `scripts/lib/fixture-signing.ts`) —
 * the same code `scripts/updater-dryrun.ts` exercises — rather than a second
 * one written for these tests.
 *
 * The case that matters most is the first one: the pubkey shipped in
 * `tauri.conf.json` is still a placeholder, so every download must fail
 * verification and nothing may be installed. That is the behaviour a maintainer
 * sees before configuring a real key.
 */
import { once } from 'node:events';
import { readFileSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import path from 'node:path';
import { gzipSync } from 'node:zlib';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { corruptBox, makeFixtureKey, signBox, signatureField } from '../../../scripts/lib/fixture-signing';
import {
  compareVersions,
  evaluateUpdate,
  parseManifest,
  publicKeyProblem,
  selectPlatform,
  type ReleaseManifest,
} from '../../../scripts/lib/updater';
import { defaultSettings } from '../../app/ipc/types';
import { useSettings } from '../../stores/settingsStore';
import type { UpdateTransport } from './transport';
import { resetLaunchCheckLatch, useUpdates } from './updatesStore';

const ARCHIVE_NAME = 'Sift.app.tar.gz';
const CURRENT_VERSION = '1.0.0';
const TARGET = 'darwin-aarch64';

vi.mock('../../app/ipc/commands', () => ({
  api: {
    system_info: vi.fn(async () => ({ version: CURRENT_VERSION, oauth_available: false, demo: false })),
    settings_set: vi.fn(async (patch: Record<string, unknown>) => patch),
    app_relaunch: vi.fn(async () => undefined),
  },
}));

vi.mock('./transport', () => ({
  UPDATE_CHECK_TIMEOUT_MS: 15_000,
  createTransport: () => fixtureTransport(),
}));

/** The pubkey the app is configured with, taken from the shipped config. */
const configuredPubkey = (
  JSON.parse(readFileSync(path.resolve(process.cwd(), 'src-tauri/tauri.conf.json'), 'utf8')) as {
    plugins: { updater: { pubkey: string } };
  }
).plugins.updater.pubkey;

const good = makeFixtureKey();
const archive = gzipSync(Buffer.from(`fake universal Sift.app bundle ${'x'.repeat(4096)}`));
const goodSignature = signatureField(signBox(good, archive, ARCHIVE_NAME));
const badSignature = signatureField(corruptBox(signBox(good, archive, ARCHIVE_NAME)));

/** Everything the server hands out, and every path it was asked for. */
const served = {
  manifest: null as unknown,
  manifestStatus: 200,
  artifact: archive,
  requests: [] as string[],
};

let base = '';
let server: Server;
/** The key the app would have been configured with for this case. */
let pubkey = configuredPubkey;
let installs = 0;

function platformUrl(): string {
  return `${base}/${ARCHIVE_NAME}`;
}

function manifestWith(signature: string): ReleaseManifest {
  return {
    version: '1.0.1',
    notes: 'Fixes IMAP reconnect storms.',
    pub_date: '2026-09-13T00:00:00Z',
    platforms: { [TARGET]: { signature, url: platformUrl() } },
  };
}

/**
 * The transport, standing in for `tauri-plugin-updater` at its boundary: it
 * fetches over real HTTP and verifies the artifact exactly as the Rust side
 * does before handing it to the installer.
 */
function fixtureTransport(): UpdateTransport {
  let manifest: ReleaseManifest | null = null;
  let artifactUrl = '';
  return {
    check: async () => {
      const response = await fetch(`${base}/latest.json`);
      if (!response.ok) {
        // The message the plugin reports when no endpoint produced a release.
        throw new Error('Could not fetch a valid release JSON from the remote');
      }
      const parsed = parseManifest(await response.json(), { allowInsecureLoopback: true });
      if (compareVersions(parsed.version, CURRENT_VERSION) <= 0) return null;
      const platform = selectPlatform(parsed, TARGET);
      if (!platform) {
        throw new Error(`the platform \`${TARGET}\` was not found in the response \`platforms\` object`);
      }
      manifest = parsed;
      artifactUrl = platform.url;
      return {
        version: parsed.version,
        date: parsed.pub_date,
        notes: parsed.notes,
      };
    },
    download: async (onProgress) => {
      const response = await fetch(artifactUrl);
      const bytes = Buffer.from(await response.arrayBuffer());
      onProgress(bytes.byteLength, bytes.byteLength);
      const decision = evaluateUpdate({
        manifest,
        currentVersion: CURRENT_VERSION,
        target: TARGET,
        pubkey,
        allowInsecureLoopback: true,
        download: { bytes },
      });
      if (!decision.install) throw new Error(decision.message);
    },
    install: async () => {
      installs += 1;
    },
    relaunch: async () => {},
    dispose: async () => {},
  };
}

beforeAll(async () => {
  server = createServer((req, res) => {
    const requested = (req.url ?? '/').split('?')[0];
    served.requests.push(requested);
    if (requested === '/latest.json') {
      res.writeHead(served.manifestStatus, { 'content-type': 'application/json' });
      res.end(JSON.stringify(served.manifest));
      return;
    }
    if (requested === `/${ARCHIVE_NAME}`) {
      res.writeHead(200, { 'content-type': 'application/octet-stream' });
      res.end(served.artifact);
      return;
    }
    res.writeHead(404).end('not found');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  base = `http://127.0.0.1:${typeof address === 'object' && address ? address.port : 0}`;
});

afterAll(async () => {
  const closed = once(server, 'close');
  server.close();
  await closed;
});

beforeEach(() => {
  served.manifest = manifestWith(goodSignature);
  served.manifestStatus = 200;
  served.artifact = archive;
  served.requests = [];
  pubkey = configuredPubkey;
  installs = 0;
  resetLaunchCheckLatch();
  useUpdates.setState({ status: { kind: 'idle' }, candidate: null, currentVersion: CURRENT_VERSION });
  useSettings.setState({ settings: { ...defaultSettings }, loaded: true });
});

describe('staged endpoint', () => {
  it('serves the manifest and the artifact the app is told to expect', async () => {
    await useUpdates.getState().check();

    expect(served.requests).toEqual(['/latest.json']);
    expect(useUpdates.getState().status.kind).toBe('available');
    expect(useUpdates.getState().candidate?.version).toBe('1.0.1');
  });

  it('refuses a correctly signed release while the configured key is the placeholder', async () => {
    // Stage a deliberately unusable key; production configuration is now valid.
    pubkey = Buffer.from('untrusted comment: replace with real updater public key').toString('base64');
    expect(publicKeyProblem(pubkey)).toMatch(/placeholder/);

    await useUpdates.getState().check();
    await useUpdates.getState().download();

    const status = useUpdates.getState().status;
    expect(status.kind).toBe('failed');
    if (status.kind !== 'failed') throw new Error('unreachable');
    expect(status.stage).toBe('download');
    expect(status.failure.code).toBe('signature');
    expect(status.failure.message).toMatch(/could not be verified/);
    expect(served.requests).toEqual(['/latest.json', `/${ARCHIVE_NAME}`]);
    expect(installs).toBe(0);

    // And installing is not reachable afterwards, however it is asked for.
    await useUpdates.getState().install();
    expect(installs).toBe(0);
  });

  it('refuses an artifact whose signature was tampered with, and says why', async () => {
    served.manifest = manifestWith(badSignature);
    pubkey = good.pubkeyField;

    await useUpdates.getState().check();
    await useUpdates.getState().download();

    const status = useUpdates.getState().status;
    expect(status.kind).toBe('failed');
    if (status.kind !== 'failed') throw new Error('unreachable');
    expect(status.failure.code).toBe('signature');
    expect(status.failure.detail).toMatch(/signature/i);
    expect(installs).toBe(0);
  });

  it('accepts a build signed by the configured key and still waits for an explicit install', async () => {
    pubkey = good.pubkeyField;

    await useUpdates.getState().check();
    expect(useUpdates.getState().status.kind).toBe('available');
    expect(installs).toBe(0);

    await useUpdates.getState().download();

    expect(useUpdates.getState().status.kind).toBe('ready');
    expect(installs).toBe(0);

    await useUpdates.getState().install();

    expect(installs).toBe(1);
    expect(useUpdates.getState().status.kind).toBe('installed');
  });

  it('reports a 404 on the manifest as "no published release yet", not as an error', async () => {
    served.manifestStatus = 404;

    await useUpdates.getState().check();

    expect(useUpdates.getState().status.kind).toBe('no-release');
    expect(useSettings.getState().settings.updatesLastCheckState).toBe('no-release');
    expect(useSettings.getState().settings.updatesLastCheckError).toBe('');
    await useUpdates.getState().download();
    expect(installs).toBe(0);
  });

  it('reports a release that is not newer as up to date, without downloading', async () => {
    served.manifest = { ...manifestWith(goodSignature), version: CURRENT_VERSION };

    await useUpdates.getState().check();

    expect(useUpdates.getState().status.kind).toBe('up-to-date');
    expect(served.requests).toEqual(['/latest.json']);
  });

  it('reports a release with no build for this Mac instead of downloading nothing', async () => {
    served.manifest = {
      ...manifestWith(goodSignature),
      platforms: { 'windows-x86_64': { signature: goodSignature, url: platformUrl() } },
    };

    await useUpdates.getState().check();

    const status = useUpdates.getState().status;
    expect(status.kind).toBe('failed');
    if (status.kind !== 'failed') throw new Error('unreachable');
    expect(status.failure.code).toBe('platform');
    expect(served.requests).toEqual(['/latest.json']);
  });
});
