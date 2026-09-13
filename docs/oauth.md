# Connecting Gmail

There are two ways Sift signs in to Gmail. Choose one for your build.

## App password (default, nothing to set up)

This is what public builds ship with, and it is what almost every user should
use.

- The user turns on 2-Step Verification and creates a 16-letter app password,
  then pastes it into Sift.
- No Google Cloud project, no OAuth client, no Google review. It works for any
  Gmail or Google Workspace account.
- Sift talks to Gmail over IMAP and SMTP directly. See the README for the
  two-minute steps.

Because this path exists, **you do not need OAuth to launch Sift publicly.**

## Sign in with Google (optional, for builds you control)

The wizard shows a "Sign in with Google instead" button only when a real OAuth
client id is compiled in. This is optional and adds a Google review step if you
want it for the general public.

### One-time setup

1. In [Google Cloud Console](https://console.cloud.google.com/), create a
   project and enable the **Gmail API**.
2. Configure the OAuth consent screen:
   - User type: **External** (or **Internal** if only your Workspace uses it).
   - App name, support email, and developer contact.
   - Scopes: `gmail.modify`, `userinfo.email`, `userinfo.profile`.
   - Add yourself (and any testers) under **Test users** while in Testing.
3. Create credentials → **OAuth client ID** → Application type **Desktop app**.
   Copy the client id and client secret. Desktop clients are treated as public,
   so the secret is embedded in the app, and Sift still uses PKCE.

### Baking it into a build

Provide both values at build time. In CI they come from repository secrets:

```sh
SIFT_GOOGLE_CLIENT_ID=…apps.googleusercontent.com \
SIFT_GOOGLE_CLIENT_SECRET=… \
pnpm tauri build
```

`system_info().oauth_available` is true only when the client id is present and
looks real, so a placeholder never shows a broken button. Locally you can put the
two variables in `.env` (gitignored).

### Launching publicly with OAuth

`gmail.modify` is a **restricted scope**. What that means in practice:

- While the consent screen is in **Testing**, only accounts added as test users
  (up to 100) can sign in. Fine for a beta, not for the public.
- To let anyone sign in, you must **publish** the app, which requires Google's
  OAuth verification for restricted scopes. Google may also require a security
  assessment depending on how the data is handled. Plan for weeks, not hours.
- A local-only app that never sends mail content to a server is the easiest case
  to justify, but the verification process is the same.

If you would rather not go through verification, ship the app-password path (the
default) and leave the Google button for builds where the client id is verified.
The two can coexist: verified builds show the button, everyone else uses an app
password.
