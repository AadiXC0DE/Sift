import { describe, it, expect, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Contact } from '../../app/ipc/types';
import type { NotificationsState } from '../mail-utilities/ipc';
import { NotificationSettings } from './NotificationSettings';
import { VipPanel } from './VipPanel';

const ipc = vi.hoisted(() => ({
  call: vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}));

vi.mock('../../app/ipc/commands', () => ({ call: ipc.call }));

interface Backend {
  state: NotificationsState;
  vips: Record<string, string[]>;
  candidates: Record<string, Contact[]>;
  calls: string[];
}

const BOSS: Contact = { email: 'boss@work.example', name: 'Dana Boss', lastUsedAt: 2, useCount: 9 };
const SAM: Contact = { email: 'sam@friend.example', name: null, lastUsedAt: 1, useCount: 1 };

let backend: Backend;

function makeBackend(overrides: Partial<NotificationsState> = {}): Backend {
  return {
    state: {
      enabled: true,
      permission: 'granted',
      filter: 'inbox',
      hideSubject: false,
      sound: 'native',
      accountIds: ['a1'],
      ...overrides,
    },
    vips: { a1: [BOSS.email], a2: [] },
    candidates: { a1: [BOSS, SAM], a2: [] },
    calls: [],
  };
}

// `Promise.withResolvers` needs ES2024 lib; this project targets ES2022.
function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

/** Stand-in for the Rust side; every branch mirrors the wire contract. */
async function handle(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  backend.calls.push(cmd);
  const a = args ?? {};
  switch (cmd) {
    case 'notifications_state':
      return backend.state;
    case 'notifications_enable':
      backend.state = { ...backend.state, enabled: a.enabled as boolean };
      return backend.state;
    case 'notifications_update': {
      // Mock transport: the caller's own field subset, as the real command
      // would receive it.
      const patch = args as Partial<NotificationsState>;
      backend.state = { ...backend.state, ...patch };
      return backend.state;
    }
    case 'vip_list':
      return backend.vips[a.accountId as string] ?? [];
    case 'vip_set': {
      const accountId = a.accountId as string;
      const email = a.email as string;
      const list = backend.vips[accountId] ?? [];
      backend.vips[accountId] = a.vip
        ? [...list.filter((e) => e !== email), email]
        : list.filter((e) => e !== email);
      return backend.vips[accountId];
    }
    case 'vip_candidates':
      return backend.candidates[a.accountId as string] ?? [];
    default:
      throw new Error(`unexpected command ${cmd}`);
  }
}

beforeEach(() => {
  backend = makeBackend();
  ipc.call.mockReset();
  ipc.call.mockImplementation(handle);
});

const vipRow = (email: string) => document.querySelector(`[data-vip="${email}"]`);
const candidateRow = (email: string) => document.querySelector(`[data-vip-candidate="${email}"]`);

describe('NotificationSettings (P8.4)', () => {
  it('saves the filter, hide-subject, sound and per-account enablement', async () => {
    const user = userEvent.setup();
    render(<NotificationSettings accountIds={['a1', 'a2']} accountNames={{ a1: 'Work', a2: 'Personal' }} />);

    await screen.findByRole('switch', { name: 'Enable notifications' });

    await user.click(screen.getByRole('switch', { name: 'Hide subject' }));
    expect(ipc.call).toHaveBeenCalledWith('notifications_update', { hideSubject: true });
    expect(screen.getByRole('switch', { name: 'Hide subject' })).toBeChecked();

    await user.click(screen.getByRole('button', { name: 'VIP' }));
    expect(ipc.call).toHaveBeenCalledWith('notifications_update', { filter: 'vip' });
    // The VIP list is the link target of the "only VIPs notify" explanation.
    expect(await screen.findByText('Recent correspondents')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'None' }));
    expect(ipc.call).toHaveBeenCalledWith('notifications_update', { sound: 'none' });

    await user.click(screen.getByRole('switch', { name: 'Notify for Personal' }));
    expect(ipc.call).toHaveBeenCalledWith('notifications_update', { accountIds: ['a1', 'a2'] });
    await user.click(screen.getByRole('switch', { name: 'Notify for Work' }));
    expect(ipc.call).toHaveBeenCalledWith('notifications_update', { accountIds: ['a2'] });

    // Nothing here may ask the system for permission.
    expect(ipc.call).not.toHaveBeenCalledWith('notifications_enable', expect.anything());
  });

  it('keeps a second account toggle made while the first write is in flight', async () => {
    const user = userEvent.setup();
    ipc.call.mockImplementation(async (cmd, args) => {
      if (cmd === 'notifications_update') await delay(60);
      return handle(cmd, args);
    });
    render(<NotificationSettings accountIds={['a1', 'a2', 'a3']} />);
    await screen.findByRole('switch', { name: 'Enable notifications' });

    await user.click(screen.getByRole('switch', { name: 'Notify for a2' }));
    await user.click(screen.getByRole('switch', { name: 'Notify for a3' }));

    expect(ipc.call).toHaveBeenCalledWith('notifications_update', { accountIds: ['a1', 'a2', 'a3'] });
    // Both overdue responses land in order; the panel ends on the last one.
    await waitFor(() => {
      expect(screen.getByRole('switch', { name: 'Notify for a2' })).toBeChecked();
      expect(screen.getByRole('switch', { name: 'Notify for a3' })).toBeChecked();
    });
  });

  it('keeps the controls visible but inert while the system blocks notifications', async () => {
    backend = makeBackend({ permission: 'denied', enabled: true });
    render(<NotificationSettings accountIds={['a1']} accountNames={{ a1: 'Work' }} />);

    expect(await screen.findByText(/Your system is blocking notifications for Sift/)).toBeInTheDocument();
    expect(screen.getByText(/Open System Settings/)).toBeInTheDocument();
    // No permission prompt on mount, and never a prompt loop.
    expect(ipc.call).not.toHaveBeenCalledWith('notifications_enable', expect.anything());

    expect(screen.getByRole('switch', { name: 'Hide subject' })).toBeInTheDocument();
    expect(document.querySelectorAll('[data-inert="true"]').length).toBeGreaterThan(0);

    fireEvent.click(screen.getByRole('button', { name: 'VIP' }));
    fireEvent.click(screen.getByRole('switch', { name: 'Hide subject' }));
    expect(ipc.call).not.toHaveBeenCalledWith('notifications_update', expect.anything());

    // The master switch stays live: after fixing System Settings it is the one
    // place that asks the system again.
    fireEvent.click(screen.getByRole('switch', { name: 'Enable notifications' }));
    await waitFor(() => expect(ipc.call).toHaveBeenCalledWith('notifications_enable', { enabled: false }));
  });

  it('says the build cannot deliver notifications and stays inert', async () => {
    backend = makeBackend({ permission: 'unsupported' });
    render(<NotificationSettings accountIds={['a1']} />);

    expect(await screen.findByText(/This build cannot deliver notifications/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('switch', { name: 'Enable notifications' }));
    fireEvent.click(screen.getByRole('button', { name: 'Off' }));
    expect(ipc.call).not.toHaveBeenCalledWith('notifications_enable', expect.anything());
    expect(ipc.call).not.toHaveBeenCalledWith('notifications_update', expect.anything());
  });

  it('degrades gracefully when the backend command is not registered', async () => {
    ipc.call.mockRejectedValueOnce(new Error('command notifications_state not found'));
    render(<NotificationSettings accountIds={['a1']} />);

    expect(
      await screen.findByText('Notification settings are not available in this build.'),
    ).toBeInTheDocument();
    expect(ipc.call).not.toHaveBeenCalledWith('notifications_enable', expect.anything());
  });
});

describe('VipPanel (P8.4)', () => {
  it('adds and removes VIPs from the list the backend returns', async () => {
    const user = userEvent.setup();
    render(<VipPanel accountIds={['a1']} accountNames={{ a1: 'Work' }} />);

    await waitFor(() => expect(vipRow(BOSS.email)).not.toBeNull());
    expect(screen.getByText('Dana Boss')).toBeInTheDocument();
    // A VIP also appears in the suggestions until it is picked.
    expect(candidateRow(BOSS.email)).toBeNull();
    expect(candidateRow(SAM.email)).not.toBeNull();
    expect(screen.getByText(SAM.email)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: `Remove ${BOSS.email}` }));
    expect(ipc.call).toHaveBeenCalledWith('vip_set', {
      accountId: 'a1',
      email: BOSS.email,
      vip: false,
    });
    await waitFor(() => expect(vipRow(BOSS.email)).toBeNull());
    //  ... and drops back into the candidate list from the returned list alone.
    await waitFor(() => expect(candidateRow(BOSS.email)).not.toBeNull());

    await user.click(screen.getByRole('button', { name: `Add ${BOSS.email} as a VIP` }));
    expect(ipc.call).toHaveBeenCalledWith('vip_set', { accountId: 'a1', email: BOSS.email, vip: true });
    await waitFor(() => expect(vipRow(BOSS.email)).not.toBeNull());
    expect(candidateRow(BOSS.email)).toBeNull();
    expect(candidateRow(SAM.email)).not.toBeNull();
  });

  it('says honestly when there is no one recent to suggest', async () => {
    render(<VipPanel accountIds={['a2']} accountNames={{ a2: 'Personal' }} />);

    expect(await screen.findByText(/No VIP senders yet/)).toBeInTheDocument();
    expect(screen.getByText(/Nobody to suggest yet/)).toBeInTheDocument();
    expect(screen.getByText('Personal')).toBeInTheDocument();
  });

  it('keeps one account usable when the other account fails to load', async () => {
    ipc.call.mockImplementation(async (cmd, args) => {
      backend.calls.push(cmd);
      const accountId = (args ?? {}).accountId as string;
      if (accountId === 'a2') throw new Error('offline');
      if (cmd === 'vip_list') return backend.vips[accountId] ?? [];
      if (cmd === 'vip_candidates') return backend.candidates[accountId] ?? [];
      throw new Error(`unexpected command ${cmd}`);
    });

    render(<VipPanel accountIds={['a1', 'a2']} />);

    expect(await screen.findByText(/VIP senders are not available for this account/)).toBeInTheDocument();
    expect(vipRow(BOSS.email)).not.toBeNull();
  });
});
