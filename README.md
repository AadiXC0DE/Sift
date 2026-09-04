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
