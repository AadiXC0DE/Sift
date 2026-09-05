import { create } from 'zustand';
import { api } from '../app/ipc/commands';
import { defaultSettings, type Settings } from '../app/ipc/types';
import { useView, type PaneLayout } from './viewStore';

interface S {
  settings: Settings;
  loaded: boolean;
  load: () => Promise<void>;
  set: (p: Partial<Settings>) => Promise<void>;
  setLocal: (p: Partial<Settings>) => void;
}

function applyTheme(s: Settings) {
  const root = document.documentElement;
  const dark =
    s.theme === 'dark' || (s.theme === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
  root.dataset.theme = dark ? 'dark' : 'light';
  root.dataset.accent = s.accent;
  root.dataset.density = s.density;
}

function applyPane(s: Settings) {
  const pane: PaneLayout = s.readingPane === 'bottom' || s.readingPane === 'off' ? s.readingPane : 'right';
  useView.getState().setPane(pane);
}

export const useSettings = create<S>((set, get) => ({
  settings: { ...defaultSettings, readingPane: 'right' },
  loaded: false,
  load: async () => {
    try {
      const raw = await api.settings_get();
      // Legacy 'ask' mode was removed: remote images now load by default
      // like any other email client. Treat stored 'ask' as 'always'.
      const remoteImages = raw.remoteImages === 'ask' ? 'always' : raw.remoteImages;
      const settings = { ...raw, remoteImages, readingPane: 'right' as const };
      set({ settings, loaded: true });
      applyTheme(settings);
      applyPane(settings);
      if (raw.readingPane !== 'right') void api.settings_set({ readingPane: 'right' });
      if (raw.remoteImages === 'ask') void api.settings_set({ remoteImages: 'always' });
    } catch {
      const settings = { ...defaultSettings, readingPane: 'right' as const };
      set({ settings, loaded: true });
      applyPane(settings);
    }
  },
  set: async (p) => {
    const prev = get().settings;
    const next = { ...prev, ...p };
    set({ settings: next });
    applyTheme(next);
    if (p.readingPane) applyPane(next);
    try {
      await api.settings_set(p);
    } catch {
      /* offline: keep local */
    }
  },
  setLocal: (p) => {
    const next = { ...get().settings, ...p };
    set({ settings: next });
    applyTheme(next);
  },
}));

export { applyTheme };
