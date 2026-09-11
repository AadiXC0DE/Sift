# Sift

Fast, quiet email for Gmail on macOS.

Sift is a keyboard-first Gmail client built as a native Mac app. A Rust core mirrors your whole mailbox into a local SQLite database, so every read comes off your disk instead of the network. It starts in about a third of a second, idles around 35 MB, and ships as a 9.8 MB download with no bundled browser.

Native to the metal. No Electron.

## Download

Get the latest build from the [releases page](https://github.com/AadiXC0DE/Sift/releases):

- macOS 13 Ventura or newer
- Apple Silicon and Intel (universal)
- Free while in beta

## Connect in about two minutes

Sift signs in with a Google app password, so there is nothing to configure and no Google Cloud project to create.

1. Turn on 2-Step Verification at <https://myaccount.google.com/signinoptions/two-step-verification> (skip if it is already on).
2. Create an app password named **Sift** at <https://myaccount.google.com/apppasswords>. Google shows a 16 letter password once. Copy it.
3. Open Sift, choose **Connect your Gmail**, enter your address, paste the password, and press **Connect**.

You are reading your inbox in under two minutes. Refresh tokens and app passwords live only in the macOS Keychain. Your mail never passes through a Sift server, because there is no Sift server.

If you want the "Sign in with Google" button instead, see [`docs/oauth.md`](docs/oauth.md) for the optional one time Google Cloud setup.

## Why people keep it open

- **Speed as a feature.** Navigation, search, and triage target under 50 ms. Opening a thread targets under 100 ms, with metadata first and the body streaming in behind it.
- **Keyboard first, mouse complete.** `j`/`k`, `e`, `#`, `s`, `h`, and a `⌘K` command palette. Every action also has a button.
- **Reads everything offline.** The full mailbox is mirrored locally with SQLite FTS5. Search is a query, not a round trip. Airplane mode is a normal state.
- **Multiple accounts as a core feature.** Personal, work, and client inboxes sync independently and show up in one unified inbox with `⌘0`.
- **Mail renders like mail.** HTML messages are sanitized once at ingest and rendered in a sandboxed frame with authored CSS, tables, and layout preserved. Plain text keeps its whitespace, and quoted replies collapse.
- **Mistakes are cheap.** Archive and send are optimistic with an undo window, and snooze is built in.
- **Quiet by default.** No telemetry, no trackers, no ads. Tokens live in the Keychain and nothing phones home.

## Shortcuts

`j`/`k` move · `e` archive · `#` trash · `s` star · `h` snooze · `l` label · `r`/`a`/`f` reply · `c` compose · `/` search · `⌘K` palette · `?` help

Every binding is listed on the [shortcuts page](https://sift.ownpath.xyz/shortcuts) and can be remapped in Settings.

## How it works

- **Rust core** (`src-tauri/`): sync engine, MIME parsing, HTML sanitizing, FTS indexing, and the outbox. Async tokio tasks per account.
- **Tauri 2 shell**: a 9.8 MB binary around the macOS WebView. Signing, updates, Keychain, notifications, and single instance handling.
- **Local first SQLite**: WAL mode with FTS5. One writer, pooled readers, keyset pagination. The UI reads from here and never waits on the network.
- **React 19 frontend** (`src/`): rendered in WKWebView. Small IPC payloads, virtualized lists, and sandboxed iframes for HTML mail.

The transport is Gmail over IMAP for app password accounts and the Gmail REST API for OAuth accounts. Both write the same local rows, so the rest of the app does not care which one is in use.

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

Sift talks directly to Google over TLS. No analytics, no crash reporting, no third party endpoints. Credentials are stored in the macOS Keychain, and the local database never contains tokens. See the [privacy page](https://sift.ownpath.xyz/privacy) for the full statement.

## Contributing

Issues and pull requests are welcome. Before opening a PR, run `pnpm lint`, `pnpm test`, and the Rust test suite, and keep the DTOs in `src-tauri/src/dto.rs` and `src/app/ipc/types.ts` in sync (a CI check enforces this).

## Credits

Built by [Aaditya](https://github.com/AadiXC0DE). The design language and performance budgets are documented in the architecture notes. Fonts, icons, and open source dependencies are listed in the app's settings.
