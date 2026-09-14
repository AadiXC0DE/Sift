import { describe, it, expect, vi, beforeEach } from 'vitest';
import React from 'react';
import { render, screen, act, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Label } from '../../app/ipc/types';
import type { MailRule, MailRuleInput, RulePreview } from '../mail-utilities/ipc';

const mocks = vi.hoisted(() => ({
  call: vi.fn(),
  labelsList: vi.fn(),
}));

vi.mock('../../app/ipc/commands', () => ({
  api: { labels_list: (accountId: string) => mocks.labelsList(accountId) },
  call: (cmd: string, args?: Record<string, unknown>) => mocks.call(cmd, args),
}));

import { RulesPanel } from './RulesPanel';

/** The fake backend reads each command's documented argument shape. */
function argsOf<T>(args: Record<string, unknown> | undefined): T {
  return (args ?? {}) as T;
}

/** Every captured argument object for one command, in call order. */
function callArgs(cmd: string): Record<string, unknown>[] {
  return mocks.call.mock.calls
    .filter((call) => call[0] === cmd)
    .map((call) => (call[1] ?? {}) as Record<string, unknown>);
}

function ruleAt(id: string, over: Partial<MailRule> = {}): MailRule {
  return {
    id,
    accountId: 'a1',
    name: 'VIP senders',
    enabled: true,
    match: 'all',
    conditions: [{ field: 'sender', op: 'contains', value: 'boss@example.com' }],
    actions: [{ kind: 'addLabel', labelId: 'l1' }],
    sortOrder: 0,
    revision: 7,
    lastError: null,
    ...over,
  };
}

function labelAt(id: string, name: string, sortOrder: number, depth?: number): Label {
  return {
    account_id: 'a1',
    id,
    name,
    kind: 'user',
    visible: true,
    unread_count: 0,
    total_count: 0,
    sort_order: sortOrder,
    depth: depth ?? 0,
  };
}

interface Gate<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Gate<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

let store: MailRule[];
let previewResult: RulePreview;
let previewGate: Gate<RulePreview> | null;

beforeEach(() => {
  cleanup();
  store = [ruleAt('r1')];
  previewResult = { count: 0, sample: [] };
  previewGate = null;
  mocks.call.mockReset();
  mocks.labelsList.mockReset();
  mocks.labelsList.mockImplementation(async () => [labelAt('l1', 'Work', 0)]);
  mocks.call.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'rules_list': {
        const { accountId } = argsOf<{ accountId: string }>(args);
        return store.filter((r) => r.accountId === accountId);
      }
      case 'rules_upsert': {
        const { rule } = argsOf<{ rule: MailRuleInput }>(args);
        const existing = rule.id ? store.find((r) => r.id === rule.id) : undefined;
        const saved: MailRule = {
          id: rule.id ?? `new-${store.length}`,
          accountId: rule.accountId,
          name: rule.name,
          enabled: rule.enabled,
          match: rule.match,
          conditions: rule.conditions,
          actions: rule.actions,
          sortOrder: rule.sortOrder ?? 0,
          revision: (existing?.revision ?? 0) + 1,
          lastError: null,
        };
        store = [...store.filter((r) => r.id !== saved.id), saved];
        return saved;
      }
      case 'rules_delete': {
        const { ruleId } = argsOf<{ ruleId: string }>(args);
        store = store.filter((r) => r.id !== ruleId);
        return undefined;
      }
      case 'rules_preview':
        return previewGate ? previewGate.promise : previewResult;
      case 'rules_apply_existing':
        return { applied: 3, skipped: 1 };
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
});

/** Renders the panel under StrictMode, the way the app mounts it, and waits for the stored rule. */
async function ready() {
  const user = userEvent.setup();
  render(
    <React.StrictMode>
      <RulesPanel accountIds={['a1']} accountNames={{ a1: 'me@example.com' }} />
    </React.StrictMode>,
  );
  await screen.findByText('VIP senders');
  return user;
}

describe('Settings -> Rules (P8.3)', () => {
  it('groups rules per account and summarises conditions and actions', async () => {
    store = [
      ruleAt('r1'),
      ruleAt('r2', {
        name: 'Bulk receipts',
        match: 'any',
        conditions: [
          { field: 'subject', op: 'contains', value: 'receipt' },
          { field: 'hasAttachment', op: 'isTrue', value: '' },
        ],
        actions: [{ kind: 'archive', labelId: null }],
        revision: 3,
        lastError: 'Label not found on the server',
      }),
    ];
    await ready();

    expect(screen.getByRole('heading', { name: 'me@example.com' })).toBeInTheDocument();
    expect(screen.getByText('Sender contains "boss@example.com" → Add label "Work"')).toBeInTheDocument();
    expect(screen.getByText('Subject contains "receipt" or Has attachment → Archive')).toBeInTheDocument();
    expect(screen.getByText('Label not found on the server').style.color).toBe('var(--danger)');
  });

  it('keeps Save disabled until the rule has a name, a matched value and an action', async () => {
    const user = await ready();
    await user.click(screen.getByRole('button', { name: 'New rule' }));

    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();

    await user.type(screen.getByLabelText('Rule name'), 'Notify me');
    await user.type(screen.getByLabelText('Condition value'), 'boss@example.com');
    // No action yet: a rule that does nothing cannot be stored.
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();

    await user.click(screen.getByRole('button', { name: 'Add action' }));
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();

    await user.clear(screen.getByLabelText('Rule name'));
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('stores a hasAttachment condition as isTrue with no value', async () => {
    const user = await ready();
    await user.click(screen.getByRole('button', { name: 'New rule' }));
    await user.type(screen.getByLabelText('Rule name'), 'Invoices');
    await user.selectOptions(screen.getByLabelText('Condition field'), 'hasAttachment');

    expect(screen.getByLabelText('Condition operator')).toHaveValue('isTrue');
    expect(screen.queryByLabelText('Condition value')).toBeNull();

    await user.click(screen.getByRole('button', { name: 'Add action' }));
    await user.click(screen.getByRole('button', { name: 'Save' }));

    const [upsert] = callArgs('rules_upsert');
    const { rule, expectedRevision } = argsOf<{ rule: MailRuleInput; expectedRevision?: number }>(upsert);
    expect(rule.conditions).toEqual([{ field: 'hasAttachment', op: 'isTrue', value: '' }]);
    expect(rule.actions).toEqual([{ kind: 'archive', labelId: null }]);
    expect(rule.name).toBe('Invoices');
    // A rule that never existed has no revision to guard against.
    expect(expectedRevision).toBeUndefined();
    expect(await screen.findByText('Has attachment → Archive')).toBeInTheDocument();
  });

  it('offers an addLabel action with the account labels indented by depth', async () => {
    mocks.labelsList.mockImplementation(async () => [
      labelAt('l1', 'Work', 0, 0),
      labelAt('l2', 'Receipts', 1, 2),
    ]);
    const user = await ready();
    await user.click(screen.getByRole('button', { name: 'New rule' }));
    await user.click(screen.getByRole('button', { name: 'Add action' }));
    await user.selectOptions(screen.getByLabelText('Action'), 'addLabel');

    const nested = screen.getByRole('option', { name: 'Receipts' });
    expect(nested.getAttribute('data-depth')).toBe('2');
    expect(nested.style.paddingLeft).toBe('32px');
  });

  it('previews before applying and never applies a rule that matches nothing', async () => {
    const gate = deferred<RulePreview>();
    previewGate = gate;
    const user = await ready();

    await user.click(screen.getByRole('button', { name: 'Run on existing mail…' }));

    const [preview] = callArgs('rules_preview');
    expect(argsOf<{ accountId: string; rule: MailRuleInput }>(preview)).toMatchObject({
      accountId: 'a1',
      rule: { id: 'r1', name: 'VIP senders', enabled: true },
    });
    expect(screen.queryByRole('button', { name: /^Apply to/ })).toBeNull();
    expect(callArgs('rules_apply_existing')).toHaveLength(0);

    await act(async () => {
      gate.resolve({ count: 0, sample: [] });
    });

    expect(await screen.findByText(/No existing messages match this rule/)).toBeInTheDocument();
    expect(screen.getByText(/Sift never deletes mail here/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Apply to/ })).toBeNull();
    expect(callArgs('rules_apply_existing')).toHaveLength(0);
  });

  it('applies to existing mail only after the count is confirmed', async () => {
    previewResult = {
      count: 3,
      sample: [
        {
          threadId: 't1',
          subject: 'Q3 invoices',
          fromName: 'Ada',
          fromEmail: 'ada@example.com',
          wouldJunk: false,
        },
      ],
    };
    const user = await ready();

    await user.click(screen.getByRole('button', { name: 'Run on existing mail…' }));
    expect(await screen.findByText('3 existing messages match.')).toBeInTheDocument();
    expect(screen.getByText('Q3 invoices')).toBeInTheDocument();
    expect(callArgs('rules_apply_existing')).toHaveLength(0);

    await user.click(screen.getByRole('button', { name: 'Apply to 3 messages' }));

    expect(callArgs('rules_apply_existing')).toEqual([{ accountId: 'a1', ruleId: 'r1', revision: 7 }]);
    expect(await screen.findByText(/Applied to 3 messages/)).toBeInTheDocument();
  });

  it('sends the stored revision when the enabled switch is toggled', async () => {
    const user = await ready();

    await user.click(screen.getByRole('switch', { name: 'VIP senders enabled' }));

    const [upsert] = callArgs('rules_upsert');
    const { rule, expectedRevision } = argsOf<{ rule: MailRuleInput; expectedRevision: number }>(upsert);
    expect(expectedRevision).toBe(7);
    expect(rule).toMatchObject({
      id: 'r1',
      accountId: 'a1',
      name: 'VIP senders',
      enabled: false,
      match: 'all',
      conditions: [{ field: 'sender', op: 'contains', value: 'boss@example.com' }],
      actions: [{ kind: 'addLabel', labelId: 'l1' }],
    });
  });

  it('deletes a rule only after the inline confirmation', async () => {
    const user = await ready();

    await user.click(screen.getByRole('button', { name: 'Delete rule VIP senders' }));
    expect(screen.getByText(/No mail is deleted/)).toBeInTheDocument();
    expect(callArgs('rules_delete')).toHaveLength(0);

    await user.click(screen.getByRole('button', { name: 'Confirm delete VIP senders' }));

    expect(callArgs('rules_delete')).toEqual([{ accountId: 'a1', ruleId: 'r1' }]);
    expect(await screen.findByText('No rules for this account yet.')).toBeInTheDocument();
  });
});
