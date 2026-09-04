import { create } from 'zustand';

export type SetupStep = 'welcome' | 'email' | 'app-password' | 'connecting';
export type SetupProgress =
  'connecting' | 'authenticating' | 'listing' | 'syncing' | 'done' | { error: string } | `error:${string}`;

export const IMAP_ERROR_COPY: Record<string, { text: string; button: string }> = {
  imap_bad_password: {
    text: "That app password didn't work. Create a fresh one and paste it again.",
    button: 'Back to app password',
  },
  imap_needs_app_password: {
    text: 'Google needs an app password here, not your normal password.',
    button: 'Back to app password',
  },
  imap_web_login_required: {
    text: 'Google wants you to sign in on the web once. Open Gmail in your browser, then try again.',
    button: 'Try again',
  },
  imap_disabled_by_admin: {
    text: 'IMAP is turned off for this account by your Google Workspace admin. Ask them to enable IMAP, or use Sign in with Google if available.',
    button: 'Try again',
  },
  imap_too_many_connections: {
    text: 'Gmail says too many apps are connected. Quit other mail apps and retry.',
    button: 'Try again',
  },
  imap_transient: {
    text: 'Gmail is having trouble right now. Retrying…',
    button: 'Try again',
  },
  imap_tls: {
    text: "The connection to Gmail isn't secure (a proxy or firewall may be interfering).",
    button: 'Try again',
  },
  offline: { text: "You're offline. Reconnect and try again.", button: 'Try again' },
  imap_all_mail_hidden: {
    text: "All Mail is hidden from IMAP. In Gmail → Settings → Labels, turn on 'Show in IMAP' for All Mail.",
    button: 'Try again',
  },
};

interface SetupState {
  step: SetupStep;
  email: string;
  appPassword: string;
  oauthAvailable: boolean;
  progress: SetupProgress | null;
  googleHosted: boolean | null;
  demoForce: boolean;
  set: (p: Partial<SetupState>) => void;
  reset: () => void;
}

export const useSetup = create<SetupState>((set) => ({
  step: 'welcome',
  email: '',
  appPassword: '',
  oauthAvailable: false,
  progress: null,
  googleHosted: null,
  demoForce: false,
  set: (p) => set(p),
  reset: () => set({ step: 'welcome', email: '', appPassword: '', progress: null, googleHosted: null }),
}));
