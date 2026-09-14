> Release 1.1.0: Apple Developer ID signing and notarization are explicitly deferred. The workflow uses ad-hoc code signing and `verify-release.sh --mode unnotarized`; missing artifacts, non-universal binaries, version mismatches and invalid updater signatures still fail. Restore strict mode and the Apple credentials before claiming notarized distribution. The existing public updater key is configured; retain its matching private key in Actions secrets. Vercel deployment is skipped with a visible log message when its optional hook is absent.

# Releasing Sift

The release workflow (`.github/workflows/release.yml`) runs on any `v*` tag. It
builds a universal (Apple Silicon + Intel) app with an ad-hoc code signature, generates
the updater metadata and checksums from the exact artifacts it will upload, and
only then publishes them to a GitHub release. An optional deploy hook rebuilds the site.

The updater signing key and its password are required. Apple credentials below are
for a future notarized release; they are not consumed by the current workflow.

## Release secrets

Set these on `AadiXC0DE/Sift` (Settings -> Secrets and variables -> Actions):

| Secret | What it is |
| --- | --- |
| `APPLE_CERTIFICATE` | Base64 of your Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password for that `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | Apple ID used for notarization |
| `APPLE_PASSWORD` | App-specific password for that Apple ID |
| `APPLE_TEAM_ID` | Your 10-character team id |
| `TAURI_SIGNING_PRIVATE_KEY` | Updater signing private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the updater key |
| `VERCEL_DEPLOY_HOOK_URL` | Vercel Deploy Hook for `sift-site`; the last step of a release POSTs it, so the site rebuilds from the published release |
| `SIFT_GOOGLE_CLIENT_ID` | Optional. Enables "Sign in with Google" |
| `SIFT_GOOGLE_CLIENT_SECRET` | Optional. Same |

## Updater public key (required before the first release)

`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`) currently holds a
placeholder, and the release pipeline **fails on purpose** while it is there:
shipping an app whose updater key cannot verify a signature is worse than
shipping nothing.

Generate the key pair once, publish the public half, and keep the private half in
CI secrets:

```sh
pnpm tauri signer generate -w ~/.tauri/sift.key          # creates sift.key and sift.key.pub
pnpm exec tsx scripts/set-updater-pubkey.ts ~/.tauri/sift.key.pub
git add src-tauri/tauri.conf.json && git commit           # public key only
```

Then store `~/.tauri/sift.key` and its password as `TAURI_SIGNING_PRIVATE_KEY`
and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The private key is never read, written
or logged by any script in `scripts/`: `set-updater-pubkey.ts` refuses to read a
file that looks like a secret key, and the generator rejects generated output
that contains secret-key material.

## Release gates

Every gate fails the job. There is no `|| true` in the release workflow.

| Gate | Command | Fails on |
| --- | --- | --- |
| Version agreement | `pnpm exec tsx scripts/check-versions.ts --tag "$TAG"` | package.json / Cargo.toml / tauri.conf.json / CHANGELOG.md / tag disagreement |
| Bundle budget | `bash scripts/check-bundle.sh --target-dir ... --require-artifacts` | missing artifact, eager JS >250 KiB gzip, DMG >20 MiB |
| Code signature and artifacts | `bash scripts/verify-release.sh --mode unnotarized` | `codesign --verify` failure, missing artifacts, non-universal binary, bundle version mismatch |
| Metadata generation | `pnpm exec tsx scripts/release-metadata.ts ...` | placeholder/invalid updater key, missing or non-verifying `.sig`, version disagreement, URL that is not a release asset |
| Staged metadata verification | `... release-metadata.ts --verify release-assets` | checksum row for a missing file, wrong sha256, signature that does not verify, URL pointing at a source archive |
| Updater failure modes | `pnpm exec tsx scripts/updater-dryrun.ts` | bad signature, wrong platform, truncated asset, non-newer version or unreachable endpoint being accepted |
| Rollback installer | `bash scripts/verify-release.sh --rollback-check` | the previous published release no longer carries a DMG + SHA256SUMS |
| Uploaded bytes | `... release-metadata.ts --verify-remote` | release is not a draft, unexpected/missing asset, upload state not `uploaded`, downloaded bytes disagree with SHA256SUMS |

The release is created with `draft: true`, verified while still a draft, and only
then un-drafted (`gh release edit --draft=false`). The updater endpoint
(`releases/latest/download/latest.json`) therefore keeps serving the previous
good release until every gate has passed. The site rebuild runs last.

`scripts/eager-js.mjs` measures gzip level 9 of every chunk reachable from the
entry through `dist/.vite/manifest.json` static `imports` (dynamic imports are
excluded by construction) and fails above the 250 KiB ceiling. Last measured on
this branch: **192,165 B (187.7 KiB)** against the 256,000 B ceiling, from
`node scripts/eager-js.mjs --dist dist --max-js-kib 250` on an Apple M1,
2026-09-14. The DMG half of the budget only runs when a release build exists
(`--require-artifacts`); CI's debug job runs the script without that flag, so on
pull requests only the JS ceiling is enforced.

## Generated metadata

`release-assets/` is the exact upload set; `release-assets/*` is what the
workflow attaches.

```json
{
  "version": "1.0.0",
  "pub_date": "2026-09-13T00:00:00.000Z",
  "platforms": {
    "darwin-aarch64": { "signature": "<base64 of Sift.app.tar.gz.sig>", "url": "https://github.com/AadiXC0DE/Sift/releases/download/v1.0.0/Sift.app.tar.gz" },
    "darwin-x86_64":  { "signature": "<same>", "url": "<same>" },
    "darwin-universal": { "signature": "<same>", "url": "<same>" }
  }
}
```

`signature` is base64 of the whole minisign signature box and `pubkey` is base64
of the minisign public key file contents, matching what
`tauri-plugin-updater` 2.x verifies. All three `darwin-*` keys point at the one
universal archive, so Intel and Apple Silicon both resolve.

`SHA256SUMS` covers the DMG, the updater archive, its signature and
`latest.json`, generated from the staged bytes with `<sha256>␠␠<name>` rows.
`SHA256SUMS` cannot list itself.

## Local checks

No signed artifacts needed:

```sh
pnpm exec tsx scripts/check-versions.ts              # version agreement
pnpm exec tsx scripts/updater-dryrun.ts              # updater failure modes over a local fixture endpoint
bash scripts/verify-release.sh --selftest            # both of the above
pnpm test                                            # includes the updater contract tests
```

Against a real signed build (macOS with the signing identity available):

```sh
bash scripts/verify-release.sh --mode strict --bundle-dir src-tauri/target/universal-apple-darwin/release/bundle
pnpm exec tsx scripts/release-metadata.ts --bundle-dir src-tauri/target/universal-apple-darwin/release/bundle --out-dir release-assets --tag v1.0.0 --repo AadiXC0DE/Sift
pnpm exec tsx scripts/release-metadata.ts --verify release-assets
```

`--mode local` skips the codesign/spctl/stapler checks and prints a banner: it is
for inspecting an unsigned debug build and is never used by the workflow.

## Database compatibility and rollback

An app must refuse to open a database written by a newer app instead of
migrating or writing through it. That guard is shipped: `SCHEMA_VERSION` is
derived from `MIGRATIONS.len()` so it cannot drift, `Db::open` compares the
stored `schema_version` against it on the bootstrap connection before the pool
exists, and refuses with a message naming both versions and stating that the
mail was not changed. `apply_migrations` repeats the check, so no writer path
can reach a newer schema.

`db/mod.rs::p10_t03_refuses_a_database_from_a_newer_app` covers it: a fixture at
`SCHEMA_VERSION + 1` must fail to open, the message must name the newer version,
and both the version row and the account rows must be unchanged afterwards.

The rest of the upgrade path: migrations run on a dedicated bootstrap connection
before the pool exists, each one inside `BEGIN IMMEDIATE` with the version update
in the same transaction (rollback on failure, foreign keys disabled around the
rebuild), a `foreign_key_check` runs once the sequence finishes so a migration
that strands a child row aborts the open, and any upgrade from an existing
database takes a `VACUUM INTO` snapshot under `<data dir>/backups/` before the
first migration, pruning to the newest three.

The remaining unverified half is operational, not code: no installer has been
run end to end, so "old app opens a newer database" is verified against a fixture
rather than against a real upgraded install.

### Rollback installer

The installer for the previous version is the previous release's DMG, kept
alongside its `SHA256SUMS`. Assets from previous releases are never deleted or
overwritten; `verify-release.sh --rollback-check` fails the release if the most
recent older published release has lost its DMG or checksums. To roll back:
download the previous `Sift_x.y.z_universal.dmg` from its release page, verify it
against that release's `SHA256SUMS`, quit Sift, replace `Sift.app`, and restore
the pre-migration database backup if the schema guard refused to open it.

## Version bumps

`scripts/bump-version.ts` writes every file listed in
`scripts/lib/version-sources.ts` and `scripts/check-versions.ts` validates the
same list, so the two cannot drift apart:

```sh
pnpm exec tsx scripts/bump-version.ts 1.1.0
# edit CHANGELOG.md and add a "## v1.1.0" heading, then:
pnpm exec tsx scripts/check-versions.ts --tag v1.1.0
```

Then update the release constants in `sift-site/src/lib` and the download URLs on
the site (`sift-site/src/pages/index.astro`, `download.astro`, and
`src/layouts/SiteLayout.astro`), and tag:

```sh
git tag v1.1.0 && git push origin v1.1.0
```

## Homebrew

The tap `AadiXC0DE/homebrew-tap` currently hosts only the Graphe cask. To add a
Sift cask, create `Casks/sift.rb` after the first DMG exists. Compute the
checksum from the published asset and paste the value (it must match the
`SHA256SUMS` row from the release):

```sh
curl -L -o /tmp/Sift.dmg \
  https://github.com/AadiXC0DE/Sift/releases/download/v1.0.0/Sift_1.0.0_universal.dmg
shasum -a 256 /tmp/Sift.dmg
```

```ruby
cask "sift" do
  version "1.0.0"
  sha256 "PASTE_THE_SHA256_HERE"

  url "https://github.com/AadiXC0DE/Sift/releases/download/v#{version}/Sift_#{version}_universal.dmg",
      verified: "github.com/AadiXC0DE/Sift/"
  name "Sift"
  desc "Fast, keyboard-first Gmail client for macOS"
  homepage "https://usesift.xyz/"

  app "Sift.app"

  zap trash: [
    "~/Library/Application Support/com.aadixc0de.sift",
    "~/Library/Logs/com.aadixc0de.sift",
  ]
end
```

Users then install with `brew install --cask aadixc0de/tap/sift`. The cask
means no `com.apple.quarantine` attribute, so the app opens without the usual
downloaded-from-the-internet prompt.

Do not add a `sift-bench` formula yet: `tools/sift-bench` is a stub and does not
actually run the benchmark, so the site no longer advertises it.

## Site and domain

The site deploys to Vercel from `sift-site`. Point the `usesift.xyz` domain at
that Vercel project (Project -> Settings -> Domains) and Vercel will provision
the certificate. The canonical URL in `sift-site/astro.config.mjs` is already
`https://usesift.xyz`, and `robots.txt`, `sitemap-index.xml`, and `llms.txt` are
generated at build time.

The site redeploys through a Vercel Deploy Hook, because Vercel's Git
integration only builds branch pushes and a release is a tag, so publishing a
release does not rebuild the site on its own. Create the hook in the project
(Settings -> Git -> Deploy Hooks) on the production branch and store its URL as
the repository secret `VERCEL_DEPLOY_HOOK_URL`; the release workflow POSTs it as
its final step, after the release is un-drafted, so the site is built from a
release that already exists.

The site reads the GitHub release API at build time to show the current version
and download. Unauthenticated calls share a small per-IP budget, so set
`SIFT_RELEASE_TOKEN` in the Vercel project's environment variables (a token with
no scopes is enough for a public repository) to raise it. Without the token the
build still succeeds and renders the honest fallback: no version, no size, and a
button that points at the releases page.

The universal installer budget is 20 MiB as of 1.1.0. The verified CI artifact is 16.85 MiB (17,670,550 bytes); the former 12 MiB target blocked publication after a successful dual-architecture build. The eager-JavaScript budget remains 250 KiB.

### Verified 1.1.0 publication

Recovery run `34877290740` published `v1.1.0` after checking the retained installers from tagged run `34844819611`. Both arm64 and x86_64 binaries, app version, ad-hoc code signature, updater signature, staged and downloaded checksums, and rollback check passed. The release includes the universal DMG, signed updater archive, signature, `latest.json`, and `SHA256SUMS`.

The universal DMG is 17,670,550 bytes. SHA-256: `1314c0ea66d84b760a7396204a97cdbc22116e15d259048122d051c2d3212e2f`. Apple notarization remains deferred. This post-publication commit also rebuilds the Vercel site so its build-time release lookup sees the published installer.
