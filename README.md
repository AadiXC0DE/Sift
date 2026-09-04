# Sift — Fast, quiet email for Gmail on macOS

Blazing-fast, keyboard-first desktop email client for Gmail / Google Workspace.
Rust core + React UI in Tauri 2. Local-first SQLite mirror, offline triage.

## Run

```sh
pnpm install
pnpm tauri dev
```

## Connect with an app password (no setup, ~2 minutes)

1. Turn on 2-Step Verification: https://myaccount.google.com/signinoptions/two-step-verification (skip if it's already on).
2. Create an app password named **Sift**: https://myaccount.google.com/apppasswords — Google shows a 16-letter password once. Copy it.
3. Open Sift → Connect your Gmail → enter your address → paste the 16-letter password → Connect. You're reading your inbox within two minutes.

Want the "Sign in with Google" button in your own builds instead? See `docs/oauth.md` (optional, one-time Google Cloud setup).

See `SIFT_SPEC.md` (not in repo) for the full build spec. Contributor overview in `docs/architecture.md`.

## Shortcuts

`j/k` move · `e` archive · `#` trash · `s` star · `h` snooze · `l` label · `r/a/f` reply · `c` compose · `/` search · `⌘K` palette · `?` help

## End-user sign-in (downloaded builds)

Public builds sign in with an app password (above) — no credentials to configure.
When a Google OAuth client ID is baked in at release time from CI secrets
(`SIFT_GOOGLE_CLIENT_ID` / `SIFT_GOOGLE_CLIENT_SECRET`, see `release.yml`),
the wizard also offers "Sign in with Google instead" (PKCE + loopback; Google
sanctions Desktop-app IDs as public by definition). Refresh tokens and app
passwords live in the Mac Keychain; nothing else leaves the machine.

Local dev builds use `.env` only for the Google button (same variable names,
both optional); without it the app runs the app-password path, test
placeholders, or `SIFT_DEMO=1` for the fictional offline mailbox.
