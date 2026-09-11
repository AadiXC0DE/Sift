# Optional: enable Sign in with Google for your own builds

Public Sift builds need nothing here. Users connect with an app password
(see README). This page is only for developers who want the
"Sign in with Google instead" button in builds they compile themselves.

One-time setup (~15 minutes):

1. Open [Google Cloud Console](https://console.cloud.google.com/), create a
   project (any name), and enable the **Gmail API**.
2. Configure the OAuth consent screen: User type **External**, app name
   **Sift**, scopes `gmail.modify`, `userinfo.email`, `userinfo.profile`.
   Add yourself as a test user (publishing/verification is only needed for
   wide distribution; app-password users never touch this).
3. Create credentials → **OAuth client ID** → Application type
   **Desktop app**. Copy the client ID and secret.
4. Export them when building:
   `SIFT_GOOGLE_CLIENT_ID=… SIFT_GOOGLE_CLIENT_SECRET=… pnpm tauri dev`
   (or add to `.env`; both variables are optional and default to test
   placeholders). The wizard shows the Google button only when an ID is
   present (`system_info().oauth_available`).
