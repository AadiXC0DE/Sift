# Sift

Fast, quiet email for Gmail on macOS. Website: <https://usesift.xyz>.

Sift is a keyboard-first Gmail client for macOS. A Rust core mirrors your mailbox into local SQLite for fast lists and search. The app uses the Mac’s built-in WKWebView, with no bundled browser engine.

## Download

[Download Sift for Mac](https://usesift.xyz/download) · [Latest GitHub release](https://github.com/AadiXC0DE/Sift/releases/latest)

- macOS 13 Ventura or newer
- Universal app for Apple Silicon and Intel
- Free while in beta

Open the DMG and drag **Sift** into **Applications**. Version 1.1.0 is not Apple Developer ID signed or notarized. Verify the download against the release’s `SHA256SUMS`; if macOS blocks opening it, use **System Settings → Privacy & Security → Open Anyway**. The in-app updater verifies updates with Sift’s separate cryptographic signing key.

## Connect with an app password

Sift signs in with a Google app password, so there is nothing to configure and no Google Cloud project to create.

1. Turn on 2-Step Verification at <https://myaccount.google.com/signinoptions/two-step-verification> (skip if it is already on).
2. Create an app password named **Sift** at <https://myaccount.google.com/apppasswords>. Google shows a 16 letter password once. Copy it.
3. Open Sift, choose **Connect your Gmail**, enter your address, paste the password, and press **Connect**.

That is the whole setup: no Google Cloud project, no OAuth consent screen, no server in the middle. Refresh tokens and app passwords live only in the macOS Keychain. Your mail never passes through a Sift server, because there is no Sift server.

If you want the "Sign in with Google" button instead, see [`docs/oauth.md`](docs/oauth.md) for the optional one time Google Cloud setup.

## Why people keep it open

- **Speed as a feature.** Lists, search and thread metadata are served from local SQLite, and a thread opens with its metadata first while the body streams in behind it. No latency figure is published yet: the release budget rows (launch, list focus, body open, archive, search) are measured on device and are listed as pending in [`docs/acceptance-dossier.md`](docs/acceptance-dossier.md).
- **Keyboard first, mouse complete.** `j`/`k`, `e`, `#`, `s`, `h`, and a `⌘K` command palette. Every action also has a button.
- **Reads offline.** Message metadata, labels and the full-text search index are mirrored locally with SQLite FTS5, and bodies are cached as you read them, so search is a query rather than a round trip. Opening a message whose body has never been fetched still needs the network, and upgrading across a rendering migration re-fetches cached bodies.
- **Multiple accounts as a core feature.** Personal, work, and client inboxes sync independently and show up in one unified inbox with `⌘0`.
- **Mail renders like mail.** HTML messages are sanitized once at ingest and rendered in a sandboxed frame with authored CSS, tables, and layout preserved. Plain text keeps its whitespace, and quoted replies collapse.
- **Mistakes are cheap, within limits.** Archive and other label actions apply locally straight away and can be undone while the operation is still queued, and a queued send can be cancelled before its deadline. Undo restores the recorded previous state. Cancelling a queued send returns it to an editable draft; permanent deletion cannot be undone. Snooze is built in.
- **Quiet by default.** No telemetry, trackers, ads, or Sift server. Credentials live in the Keychain. New installations ask before loading remote images; you control that policy in Settings → Privacy. Automatic GitHub update checks can be disabled in Settings → Updates.

## Shortcuts

`j`/`k` move · `e` archive · `#` trash · `s` star · `h` snooze · `l` label · `r`/`a`/`f` reply · `c` compose · `/` search · `⌘K` palette · `?` help

Every binding is listed in the app under Settings and can be remapped.

## How it works

- **Rust core** (`src-tauri/`): sync engine, MIME parsing, HTML sanitizing, FTS indexing, and the outbox. Async tokio tasks per account, cancelled and awaited when an account is removed.
- **Tauri 2 shell**: a thin native shell around the macOS WebView, with no bundled browser engine. Signing, updates, Keychain, notifications, and single instance handling.
- **Local first SQLite**: WAL mode with FTS5. One write lane, pooled readers, keyset pagination. The UI reads from here and never waits on the network.
- **React 19 frontend** (`src/`): rendered in WKWebView. Small IPC payloads, virtualized lists, and sandboxed iframes for HTML mail.

The transport is Gmail over IMAP for app password accounts and the Gmail REST API for OAuth accounts. Both write the same local rows, so the rest of the app does not care which one is in use. Each signed-in account holds at most three IMAP connections: the sync worker, the IDLE slot, and one bounded foreground lease that user-initiated fetches borrow and release.

More detail in [`docs/architecture.md`](docs/architecture.md).

## Development

Requirements: macOS, Node 20+, pnpm 9+, and the Rust stable toolchain.

```sh
pnpm install
pnpm tauri dev
```

Useful commands:

```sh
pnpm lint          # eslint, tsc, prettier, DTO sync, color tokens
pnpm test          # frontend unit tests (vitest)
pnpm e2e           # end to end tests (playwright)
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
```

There is also a Rust test runner via `just` (`just test`, `just lint`, `just build`).

To run the offline demo mailbox without connecting an account, start the app with `SIFT_DEMO=1`. The demo seeds a fictional team inbox that exercises the real sync, render, and triage paths.

## Privacy

Sift talks directly to Google over TLS for mail and to GitHub for update checks and downloads. There is no analytics service, crash reporting service, or Sift backend. Credentials are stored in the macOS Keychain, never in the local mailbox database. Sender-hosted images load only when your remote-content policy allows them; new installations start with **Ask**. See [Privacy](https://usesift.xyz/privacy) for details.

## Contributing

Issues and pull requests are welcome. Before opening a PR, run `pnpm lint`, `pnpm test`, and the Rust test suite, and keep the DTOs in `src-tauri/src/dto.rs` and `src/app/ipc/types.ts` in sync (a CI check enforces this).
