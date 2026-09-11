# Releasing Sift

The release workflow (`.github/workflows/release.yml`) runs on any `v*` tag. It
builds a universal (Apple Silicon + Intel) app, signs and notarizes it, attaches
the DMG, updater artifacts, and checksums to the GitHub release, then pings the
site to rebuild.

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

Generate the updater keypair once and put the public key in
`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`), then store the private
key and its password as the two `TAURI_SIGNING_*` secrets:

```sh
pnpm tauri signer generate -w ~/.tauri/sift.key
```

## Cutting a release

1. Update the version in `package.json`, `src-tauri/tauri.conf.json`, and
   `src-tauri/Cargo.toml`.
2. Update the release constants in `sift-site/src/lib` and the download URLs on
   the site (`sift-site/src/pages/index.astro`, `download.astro`, and
   `src/layouts/SiteLayout.astro`).
3. Tag and push:
   ```sh
   git tag v1.0.0 && git push origin v1.0.0
   ```
4. Watch the run. When it is green, the DMG and checksums are on the release
   and the site has been rebuilt.

## Homebrew

The tap `AadiXC0DE/homebrew-tap` currently hosts only the Graphe cask. To add a
Sift cask, create `Casks/sift.rb` after the first DMG exists. Compute the
checksum from the published asset and paste the value:

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
