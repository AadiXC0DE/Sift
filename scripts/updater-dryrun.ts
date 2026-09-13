// Dry-run harness for the signed-update path.
//
//   pnpm exec tsx scripts/updater-dryrun.ts
//
// Stages a fixture endpoint over real HTTP on localhost, serves a synthetic
// minisign-signed updater archive plus latest.json, and drives the update
// decision for every failure mode the release gates must survive. Each
// rejection must leave the simulated current install untouched. The fixture
// key pair is generated in memory per run and is not a production key.
import { once } from 'node:events';
import { createServer, type Server } from 'node:http';
import { gzipSync } from 'node:zlib';
import { corruptBox, makeFixtureKey, signBox, signatureField as encodeBox } from './lib/fixture-signing';
import { buildManifest, sha256 } from './lib/release-artifacts';
import { evaluateUpdate, type RejectCode, type UpdateDecision } from './lib/updater';

const CURRENT_VERSION = '1.0.0';
const ARCHIVE_NAME = 'Sift.app.tar.gz';

type Served = {
  manifest: unknown;
  served: Buffer;
  status: number;
};

async function startFixture(archive: Buffer): Promise<{ server: Server; base: string; state: Served }> {
  const state: Served = { manifest: null, served: archive, status: 200 };
  const server = createServer((req, res) => {
    const path = (req.url ?? '/').split('?')[0];
    if (path === '/latest.json') {
      res.writeHead(state.status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(state.manifest));
      return;
    }
    if (path === `/${ARCHIVE_NAME}`) {
      res.writeHead(200, { 'content-type': 'application/octet-stream' });
      res.end(state.served);
      return;
    }
    res.writeHead(404).end('not found');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : 0;
  return { server, base: `http://127.0.0.1:${port}`, state };
}

async function fetchDecision(base: string, target: string, pubkey: string, expectedSha256?: string): Promise<UpdateDecision> {
  let manifestText: unknown;
  try {
    const response = await fetch(`${base}/latest.json`);
    if (!response.ok) return { install: false, code: 'unreachable', message: `metadata endpoint returned ${response.status}` };
    manifestText = JSON.parse(await response.text());
  } catch (err) {
    return { install: false, code: 'unreachable', message: `metadata endpoint failed: ${(err as Error).message}` };
  }
  const url = (manifestText as { platforms?: Record<string, { url?: string }> }).platforms?.[target]?.url;
  let download: { bytes: Buffer; expectedSha256?: string } | null = null;
  if (url) {
    try {
      const asset = await fetch(url);
      if (!asset.ok) throw new Error(`asset returned ${asset.status}`);
      download = { bytes: Buffer.from(await asset.arrayBuffer()), expectedSha256 };
    } catch {
      download = null;
    }
  }
  return evaluateUpdate({
    manifest: manifestText,
    currentVersion: CURRENT_VERSION,
    target,
    pubkey,
    allowInsecureLoopback: true,
    download,
  });
}

type Scenario = {
  name: string;
  expect: RejectCode | 'ok';
  run: (base: string, state: Served, pubkey: string) => Promise<UpdateDecision>;
};

async function main(): Promise<void> {
  const key = makeFixtureKey();
  const otherKey = makeFixtureKey();
  const archive = gzipSync(Buffer.from(`fake universal Sift.app bundle ${'x'.repeat(4096)}`));
  const signatureBox = signBox(key, archive, ARCHIVE_NAME);
  const signatureField = encodeBox(signatureBox);
  const { base, server, state } = await startFixture(archive);

  const platformFor = (endpoint: string, signature: string) => ({
    'darwin-aarch64': { signature, url: `${endpoint}/${ARCHIVE_NAME}` },
  });
  // Same shape the pipeline generates; only the host is the fixture endpoint.
  const validManifest = {
    ...buildManifest({
      version: '1.0.1',
      repo: 'AadiXC0DE/Sift',
      tag: 'v1.0.1',
      archiveName: ARCHIVE_NAME,
      signatureBox,
      pubDate: '2026-09-13T00:00:00Z',
    }),
    platforms: platformFor(base, signatureField),
  };

  const corruptSignature = encodeBox(corruptBox(signatureBox));

  const scenarios: Scenario[] = [
    {
      name: 'valid-signed-update',
      expect: 'ok',
      run: async (endpoint) => fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField, sha256(archive)),
    },
    {
      name: 'valid-update-legacy-Ed-signature',
      expect: 'ok',
      run: async (endpoint, fixture) => {
        const legacy = encodeBox(signBox(key, archive, ARCHIVE_NAME, false));
        fixture.manifest = { ...validManifest, platforms: platformFor(endpoint, legacy) };
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField, sha256(archive));
      },
    },
    {
      name: 'bad-signature',
      expect: 'bad-signature',
      run: async (endpoint, fixture) => {
        fixture.manifest = { ...validManifest, platforms: platformFor(endpoint, corruptSignature) };
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField, sha256(archive));
      },
    },
    {
      name: 'signature-from-unknown-key',
      expect: 'bad-signature',
      run: async (endpoint) => fetchDecision(endpoint, 'darwin-aarch64', otherKey.pubkeyField, sha256(archive)),
    },
    {
      name: 'placeholder-pubkey-refuses-everything',
      expect: 'bad-signature',
      run: async (endpoint) =>
        fetchDecision(
          endpoint,
          'darwin-aarch64',
          Buffer.from('untrusted comment: replace with real updater public key', 'utf8').toString('base64'),
          sha256(archive),
        ),
    },
    {
      name: 'wrong-platform',
      expect: 'wrong-platform',
      run: async (endpoint, fixture) => {
        fixture.manifest = { version: '1.0.1', platforms: { 'win32-x64': { signature: signatureField, url: `${endpoint}/${ARCHIVE_NAME}` } } };
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField);
      },
    },
    {
      name: 'truncated-asset-with-checksums',
      expect: 'truncated-asset',
      run: async (endpoint, fixture) => {
        fixture.served = archive.subarray(0, Math.floor(archive.length * 0.6));
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField, sha256(archive));
      },
    },
    {
      name: 'truncated-asset-without-checksums',
      expect: 'bad-signature',
      run: async (endpoint, fixture) => {
        fixture.served = archive.subarray(0, 32);
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField);
      },
    },
    {
      name: 'bad-version-not-newer',
      expect: 'bad-version',
      run: async (endpoint, fixture) => {
        fixture.manifest = { ...validManifest, version: CURRENT_VERSION, platforms: platformFor(endpoint, signatureField) };
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField, sha256(archive));
      },
    },
    {
      name: 'bad-version-downgrade',
      expect: 'bad-version',
      run: async (endpoint, fixture) => {
        fixture.manifest = { ...validManifest, version: '0.9.9', platforms: platformFor(endpoint, signatureField) };
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField, sha256(archive));
      },
    },
    {
      name: 'unreachable-metadata-endpoint',
      expect: 'unreachable',
      run: async () => fetchDecision('http://127.0.0.1:9', 'darwin-aarch64', key.pubkeyField),
    },
    {
      name: 'unreachable-asset-url',
      expect: 'unreachable',
      run: async (endpoint, fixture) => {
        fixture.manifest = { ...validManifest, platforms: { 'darwin-aarch64': { signature: signatureField, url: `${endpoint}/missing.tar.gz` } } };
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField, sha256(archive));
      },
    },
    {
      name: 'malformed-metadata',
      expect: 'bad-metadata',
      run: async (endpoint, fixture) => {
        fixture.manifest = { version: 'not-semver', platforms: {} };
        return fetchDecision(endpoint, 'darwin-aarch64', key.pubkeyField);
      },
    },
  ];

  const rows: string[][] = [];
  let failures = 0;
  for (const scenario of scenarios) {
    state.manifest = validManifest;
    state.served = archive;
    state.status = 200;
    const decision = await scenario.run(base, state, key.pubkeyField);
    // The app only changes what is installed when the decision says so.
    let installedVersion = CURRENT_VERSION;
    if (decision.install) installedVersion = decision.version;
    const expectedOk = scenario.expect === 'ok';
    const ok = expectedOk === decision.install && (expectedOk || decision.code === scenario.expect);
    if (!ok) failures += 1;
    rows.push([
      scenario.name,
      scenario.expect,
      decision.install ? 'install' : `keep/${decision.code}`,
      installedVersion,
      ok ? 'PASS' : 'FAIL',
    ]);
  }
  const closed = once(server, 'close');
  server.close();
  await closed;

  const header = 'scenario|expected|decision|installed|result'.split('|');
  const widths = header.map((cell, index) => Math.max(cell.length, ...rows.map((row) => row[index].length)));
  console.log(header.map((cell, index) => cell.padEnd(widths[index])).join('  '));
  for (const row of rows) console.log(row.map((cell, index) => cell.padEnd(widths[index])).join('  '));
  if (failures > 0) {
    console.error(`updater dry-run: ${failures} scenario(s) did not behave as required`);
    process.exit(1);
  }
  console.log(`updater dry-run ok: ${rows.length} scenarios, every rejected update left ${CURRENT_VERSION} installed`);
}

main().catch((err: unknown) => {
  console.error(`updater dry-run: ${(err as Error).message}`);
  process.exit(1);
});
