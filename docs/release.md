# Releasing Sift

The release workflow (`.github/workflows/release.yml`) runs on any `v*` tag. It
builds a universal (Apple Silicon + Intel) app, signs and notarizes it, generates
the updater metadata and checksums from the exact artifacts it will upload, and
only then publishes them to a GitHub release, which pings the site to rebuild.

Nothing is published until the required repository secrets exist. Without them a
tagged build fails at the signing step, so do not push a `v*` tag until the
secrets below are set.

## Required secrets

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
| Bundle budget | `bash scripts/check-bundle.sh --target-dir ... --require-artifacts` | missing artifact, eager JS >250 KiB gzip, DMG >12 MiB |
| Code signature + notarization | `bash scripts/verify-release.sh --mode strict` | `codesign --verify` failure, `spctl` rejection, unstapled app or DMG, non-universal binary, bundle version mismatch |
| Metadata generation | `pnpm exec tsx scripts/release-metadata.ts ...` | placeholder/invalid updater key, missing or non-verifying `.sig`, version disagreement, URL that is not a release asset |
| Staged metadata verification | `... release-metadata.ts --verify release-assets` | checksum row for a missing file, wrong sha256, signature that does not verify, URL pointing at a source archive |
| Updater failure modes | `pnpm exec tsx scripts/updater-dryrun.ts` | bad signature, wrong platform, truncated asset, non-newer version or unreachable endpoint being accepted |
| Rollback installer | `bash scripts/verify-release.sh --rollback-check` | the previous published release no longer carries a DMG + SHA256SUMS |
| Uploaded bytes | `... release-metadata.ts --verify-remote` | release is not a draft, unexpected/missing asset, upload state not `uploaded`, downloaded bytes disagree with SHA256SUMS |

The release is created with `draft: true`, verified while still a draft, and only
then un-drafted (`gh release edit --draft=false`). The updater endpoint
(`releases/latest/download/latest.json`) therefore keeps serving the previous
good release until every gate has passed. The site rebuild runs last.

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
migrating or writing through it. `src-tauri/src/db/mod.rs` does not guard this
today: `run_migrations` silently applies no migrations when
`schema_version` is higher than the newest known migration, then continues.

Required change (Rust side, owned by the DB workstream):

1. Add `pub const SCHEMA_VERSION: i64 = 6;` next to `MIGRATIONS` and bump it in
   the same commit as any new migration.
2. In `Db::open`, before `apply_pragmas`/the pooled connection (setting
   `journal_mode=WAL` itself writes to the file), probe the existing file with a
   read-only connection and, when `schema_version > SCHEMA_VERSION`, return
   `SiftError::App { code: "db_schema_too_new", retryable: false }` with a message
   that names both versions and says the database was left untouched.
3. Repeat the same check inside `run_migrations` after reading `v` (defense in
   depth) and copy `sift.db` to `sift.db.pre-v{old}.bak` before applying any
   migration, so a user can roll back by reinstalling the older DMG and restoring
   that file.
4. Add a regression test: create a fixture database with
   `INSERT INTO schema_version VALUES (999)`, assert `Db::open` fails with the
   "newer version" message, and assert the file bytes are unchanged.

Until (2) lands, the schema guard is unenforced: this document describes the
required behaviour, not a shipped one.

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
