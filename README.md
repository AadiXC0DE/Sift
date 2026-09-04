# Sift — Fast, quiet email for Gmail on macOS

Blazing-fast, keyboard-first desktop email client for Gmail / Google Workspace.
Rust core + React UI in Tauri 2. Local-first SQLite mirror, offline triage.

## Run

```sh
cp .env.example .env   # fill SIFT_GOOGLE_CLIENT_ID / SECRET (Google Cloud → Desktop app)
pnpm install
pnpm tauri dev
```

See `SIFT_SPEC.md` (not in repo) for the full build spec. Contributor overview in `docs/architecture.md`.

## Shortcuts

`j/k` move · `e` archive · `#` trash · `s` star · `h` snooze · `l` label · `r/a/f` reply · `c` compose · `/` search · `⌘K` palette · `?` help

## End-user sign-in (downloaded builds)

DMG users never touch credentials. The Google OAuth client ID/secret are
baked into the binary at release time from CI secrets (`SIFT_GOOGLE_CLIENT_ID`
/ `SIFT_GOOGLE_CLIENT_SECRET`, see `release.yml`), which Google sanctions for
Desktop-app clients (public by definition; PKCE + loopback protect the flow).
The user clicks Connect → approves in the browser → uses the app. Refresh
tokens live in their Keychain; nothing else leaves the machine.

Local dev builds use `.env` (same variable names); without it the app runs on
test placeholders (OAuth will fail at Google) or `SIFT_DEMO=1` for the
fictional offline mailbox.
