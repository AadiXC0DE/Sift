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
  const pane = s.readingPane;
  if (pane === 'right' || pane === 'bottom' || pane === 'off') {
    useView.getState().setPane(pane as PaneLayout);
  }
}

export const useSettings = create<S>((set, get) => ({
  settings: defaultSettings,
  loaded: false,
  load: async () => {
    try {
      const settings = await api.settings_get();
      set({ settings, loaded: true });
      applyTheme(settings);
      applyPane(settings);
    } catch {
      set({ loaded: true });
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
