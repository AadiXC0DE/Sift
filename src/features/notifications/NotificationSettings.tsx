import React from 'react';
import { toast } from 'sonner';
import { utilities } from '../mail-utilities/ipc';
import type { NotificationsState } from '../mail-utilities/ipc';
import { Segmented } from '../../ui/Segmented';
import { Switch } from '../../ui/Switch';
import { VipPanel } from './VipPanel';

const FILTER_OPTIONS: { value: NotificationsState['filter']; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'inbox', label: 'Inbox' },
  { value: 'vip', label: 'VIP' },
];

const SOUND_OPTIONS: { value: NotificationsState['sound']; label: string }[] = [
  { value: 'none', label: 'None' },
  { value: 'native', label: 'Native' },
];

/** The mutable fields of `notifications_update`; `enabled` belongs to `notifications_enable`. */
type UpdatePatch = {
  filter?: NotificationsState['filter'];
  hideSubject?: boolean;
  sound?: NotificationsState['sound'];
  accountIds?: string[];
};

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
      <span style={{ display: 'flex', flexDirection: 'column' }}>
        <span style={{ fontSize: 13 }}>{label}</span>
        {hint && <span style={{ fontSize: 11, color: 'var(--fg-3)' }}>{hint}</span>}
      </span>
      {children}
    </div>
  );
}

/**
 * P8.4 — controls stay visible but unusable while the OS or the build cannot
 * deliver anything, so the panel never pretends a switch does something.
 */
function Inert({ inert, children }: { inert: boolean; children: React.ReactNode }) {
  if (!inert) return <>{children}</>;
  return (
    <div
      data-inert="true"
      aria-disabled="true"
      style={{ pointerEvents: 'none', opacity: 0.55 }}
      title="Unavailable until the system allows notifications"
    >
      {children}
    </div>
  );
}

/**
 * P8.4 — notification preferences. Rust owns delivery and permission:
 * `notifications_enable` is called only from the master switch below, never
 * when mail arrives, so a denied user is never asked again in a loop.
 */
export function NotificationSettings({
  accountIds,
  accountNames,
}: {
  accountIds: string[];
  accountNames?: Record<string, string>;
}) {
  const [state, setState] = React.useState<NotificationsState | null>(null);
  const [unavailable, setUnavailable] = React.useState(false);

  React.useEffect(() => {
    let alive = true;
    utilities
      .notifications_state()
      .then((s) => {
        if (alive) setState(s);
      })
      .catch(() => {
        // The backend command is not registered in this build; show the panel
        // without inventing values instead of failing the Settings dialog.
        if (alive) setUnavailable(true);
      });
    return () => {
      alive = false;
    };
  }, []);

  if (!state) {
    return (
      <div role="status" style={{ fontSize: 13, color: 'var(--fg-2)' }}>
        {unavailable
          ? 'Notification settings are not available in this build.'
          : 'Loading notification settings…'}
      </div>
    );
  }

  const blocked = state.permission === 'denied';
  const unsupported = state.permission === 'unsupported';
  // Turning notifications on is how the system gets asked once, so the master
  // switch stays usable while denied (the user may have just fixed Settings).
  const masterInert = unsupported;
  const othersInert = blocked || unsupported;

  // Exactly the fields `notifications_update` accepts; `notifications_enable`
  // is the only command that may change `enabled`/`permission`.
  const save = async (patch: UpdatePatch) => {
    if (othersInert) return;
    // Apply locally first so a fast second toggle builds on this change, then
    // adopt the state the backend returns as authoritative.
    setState((prev) => (prev ? { ...prev, ...patch } : prev));
    try {
      setState(await utilities.notifications_update(patch));
    } catch {
      toast.error('Could not save notification settings');
      // Re-read so a failed write never leaves the panel showing a lie.
      try {
        setState(await utilities.notifications_state());
      } catch {
        // The backend is unreachable in both directions; say so plainly.
        setState(null);
        setUnavailable(true);
      }
    }
  };

  const setEnabled = async (enabled: boolean) => {
    if (masterInert) return;
    try {
      setState(await utilities.notifications_enable({ enabled }));
    } catch {
      toast.error('Could not change notification permission');
    }
  };

  // VIPs are edited for the accounts that can actually notify; when none are
  // included the full account list is offered so the setting is not a dead end.
  const vipAccountIds = state.accountIds.length > 0 ? state.accountIds : accountIds;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      <Row label="Enable notifications" hint="Delivered by the system, never duplicated in the app">
        <Inert inert={masterInert}>
          <Switch
            checked={state.enabled}
            ariaLabel="Enable notifications"
            onChange={(v) => void setEnabled(v)}
          />
        </Inert>
      </Row>

      {blocked && (
        <div
          role="status"
          style={{
            fontSize: 12,
            color: 'var(--fg-2)',
            border: '1px solid var(--warning)',
            borderRadius: 'var(--r-md)',
            padding: '8px 10px',
          }}
        >
          Your system is blocking notifications for Sift. Open System Settings → Notifications, allow Sift
          there, then switch notifications on again here.
        </div>
      )}
      {unsupported && (
        <div
          role="status"
          style={{
            fontSize: 12,
            color: 'var(--fg-2)',
            border: '1px solid var(--border)',
            borderRadius: 'var(--r-md)',
            padding: '8px 10px',
          }}
        >
          This build cannot deliver notifications on this system. The choices below are kept for reference.
        </div>
      )}

      <Inert inert={othersInert}>
        <Row label="Notify me about">
          <Segmented
            options={FILTER_OPTIONS}
            value={state.filter}
            onChange={(f) => void save({ filter: f })}
          />
        </Row>
      </Inert>

      {state.filter === 'vip' && (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 8,
            border: '1px solid var(--border)',
            borderRadius: 'var(--r-md)',
            padding: '10px 12px',
          }}
        >
          <div style={{ fontSize: 12, color: 'var(--fg-2)' }}>
            Only the senders below notify; every other message stays quiet, even in the Inbox. VIPs come from
            mail Sift has already seen, never from your Contacts.
          </div>
          <VipPanel accountIds={vipAccountIds} accountNames={accountNames} />
        </div>
      )}

      <Inert inert={othersInert}>
        <Row label="Hide subject" hint="The banner shows the sender only">
          <Switch
            checked={state.hideSubject}
            ariaLabel="Hide subject"
            onChange={(v) => void save({ hideSubject: v })}
          />
        </Row>
      </Inert>

      <Inert inert={othersInert}>
        <Row label="Sound" hint="Native uses the system notification sound">
          <Segmented options={SOUND_OPTIONS} value={state.sound} onChange={(s) => void save({ sound: s })} />
        </Row>
      </Inert>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <span style={{ fontSize: 11, color: 'var(--fg-3)', textTransform: 'uppercase' }}>Accounts</span>
        {accountIds.length === 0 && (
          <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>No accounts to notify about yet.</div>
        )}
        {accountIds.map((id) => (
          <Inert key={id} inert={othersInert}>
            <Row label={accountNames?.[id] ?? id}>
              <Switch
                checked={state.accountIds.includes(id)}
                ariaLabel={`Notify for ${accountNames?.[id] ?? id}`}
                onChange={(v) => {
                  const next = v ? [...state.accountIds, id] : state.accountIds.filter((a) => a !== id);
                  void save({ accountIds: next });
                }}
              />
            </Row>
          </Inert>
        ))}
      </div>
    </div>
  );
}
