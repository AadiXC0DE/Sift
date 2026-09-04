import { useEffect } from 'react';
import { on } from '../../app/ipc/events';
import { useSettings } from '../../stores/settingsStore';

export function useNotifier(onOpenThread: (accountId: string, threadId: string) => void) {
  const settings = useSettings((s) => s.settings);
  useEffect(() => {
    let unsub = () => {};
    on<{ account_id: string; thread_id: string; from: string; subject: string }>(
      'notify:new-mail',
      async (p) => {
        const { account_id, thread_id, from, subject } = p as unknown as {
          account_id: string;
          thread_id: string;
          from: string;
          subject: string;
        };
        if (settings.notifications === 'off') return;
        try {
          const { isPermissionGranted, requestPermission, sendNotification } =
            await import('@tauri-apps/plugin-notification');
          let granted = await isPermissionGranted();
          if (!granted) granted = (await requestPermission()) === 'granted';
          if (granted) {
            sendNotification({ title: from, body: subject });
            if (settings.sound !== 'off') {
              const ctx = new AudioContext();
              const osc = ctx.createOscillator();
              osc.frequency.value = 880;
              osc.connect(ctx.destination);
              osc.start();
              osc.stop(ctx.currentTime + 0.04);
            }
          }
        } catch {
          /* noop */
        }
        void onOpenThread;
        void account_id;
        void thread_id;
      },
    )
      .then((u) => {
        unsub = u;
      })
      .catch(() => {});
    return () => unsub();
  }, [settings.notifications, settings.sound, onOpenThread]);
}
