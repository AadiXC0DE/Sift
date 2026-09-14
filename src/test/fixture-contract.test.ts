/**
 * Contract check between the E2E fixture backend and the IPC DTO mirror.
 *
 * `e2e/fixture/backend.ts` stands in for the Rust command surface in the
 * browser suite, so its serialized replies must match `src/app/ipc/types.ts`
 * exactly: no missing required field, and no field the DTO does not declare
 * (which would let a test pass against a shape production never sends).
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { invokeFixture, setDatabase } from '../../e2e/fixture/backend';
import { seedDatabase } from '../../e2e/fixture/dataset';

/** Vitest runs from the repository root; the DTO mirror is the contract source. */
const TYPES_SOURCE = readFileSync(path.resolve(process.cwd(), 'src/app/ipc/types.ts'), 'utf8');

/** `name -> optional` for a top-level `export interface`. */
function interfaceMembers(name: string): Map<string, boolean> {
  const match = new RegExp(`export interface ${name}\\s*\\{([\\s\\S]*?)\\n\\}`).exec(TYPES_SOURCE);
  if (!match) throw new Error(`interface ${name} not found in src/app/ipc/types.ts`);
  const body = match[1].replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
  const members = new Map<string, boolean>();
  for (const line of body.split('\n')) {
    const member = /^\s*([A-Za-z_$][\w$]*)(\?)?\s*:/.exec(line);
    if (member) members.set(member[1], !!member[2]);
  }
  return members;
}

function expectShape(value: unknown, iface: string, where: string): void {
  const declared = interfaceMembers(iface);
  const actual = Object.keys(value as Record<string, unknown>);
  const undeclared = actual.filter((k) => !declared.has(k));
  expect(undeclared, `${where}: fields not declared by ${iface}`).toEqual([]);
  const missing = [...declared].filter(([k, optional]) => !optional && !actual.includes(k)).map(([k]) => k);
  expect(missing, `${where}: required ${iface} fields missing`).toEqual([]);
}

function expectEachShape(values: unknown[], iface: string, where: string): void {
  expect(values.length, `${where}: expected at least one value`).toBeGreaterThan(0);
  for (const value of values) expectShape(value, iface, where);
}

describe('fixture backend IPC contract', () => {
  it('serializes accounts and labels with the declared DTO fields', async () => {
    setDatabase(seedDatabase('default'));
    expectEachShape((await invokeFixture('accounts_list')) as unknown[], 'Account', 'accounts_list');
    expectEachShape(
      (await invokeFixture('labels_list', { accountId: 'acc-a' })) as unknown[],
      'Label',
      'labels_list',
    );
  });

  it('serializes list pages with the declared ThreadsPage/ThreadRow fields and honours the cursor', async () => {
    setDatabase(seedDatabase('default'));
    setDatabase(seedDatabase('default'));
    const page = (await invokeFixture('threads_query', {
      query: { accountIds: ['acc-a', 'acc-b'], view: { kind: 'inbox' }, limit: 100 },
    })) as { rows: unknown[]; nextCursor?: string };
    expectShape(page, 'ThreadsPage', 'threads_query');
    expectEachShape(page.rows, 'ThreadRow', 'threads_query.rows');
    expect(page.nextCursor, 'first page of a 400-row inbox has a cursor').toBeTruthy();

    const next = (await invokeFixture('threads_query', {
      query: { accountIds: ['acc-a', 'acc-b'], view: { kind: 'inbox' }, limit: 100, cursor: page.nextCursor },
    })) as { rows: { id: string }[] };
    const firstIds = new Set((page.rows as { id: string }[]).map((r) => r.id));
    expect(next.rows.length).toBe(100);
    expect(
      next.rows.some((r) => firstIds.has(r.id)),
      'page two repeats page one',
    ).toBe(false);
  });

  it('serializes thread detail, message bodies and messages with the declared fields', async () => {
    setDatabase(seedDatabase('default'));
    const detail = (await invokeFixture('thread_get', { accountId: 'acc-a', threadId: 'blue-00' })) as {
      messages: unknown[];
    };
    expectShape(detail, 'ThreadDetail', 'thread_get');
    expectEachShape(detail.messages, 'MessageMeta', 'thread_get.messages');
    expectShape(
      await invokeFixture('message_body', { messageId: 'blue-00-m1' }),
      'MessageBody',
      'message_body',
    );
  });

  it('serializes drafts, settings, storage usage and contacts with the declared fields', async () => {
    setDatabase(seedDatabase('default'));
    const draft = await invokeFixture('drafts_upsert', {
      draft: {
        accountId: 'acc-a',
        mode: 'new',
        toJson: [{ e: 'ben@example.test' }],
        ccJson: [],
        bccJson: [],
        subject: 'contract',
        bodyHtml: '<p>x</p>',
        attachmentsJson: [],
      },
    });
    expectShape(draft, 'Draft', 'drafts_upsert');
    expectShape(await invokeFixture('settings_get'), 'Settings', 'settings_get');
    expectShape(await invokeFixture('storage_usage'), 'StorageUsage', 'storage_usage');
    expectEachShape(
      (await invokeFixture('contacts_suggest', { accountId: 'acc-a', q: 'ada', limit: 5 })) as unknown[],
      'Contact',
      'contacts_suggest',
    );
    expectEachShape((await invokeFixture('sync_status')) as unknown[], 'SyncStatus', 'sync_status');
    expectEachShape(
      (await invokeFixture('connectivity_state')) as unknown[],
      'ConnectivityState',
      'connectivity_state',
    );
    expectShape(
      await invokeFixture('attachments_save_all', { accountId: 'acc-a', messageId: 'blue-00-m1' }),
      'SaveAllResult',
      'attachments_save_all',
    );
  });

  it('applies action transitions to the stored thread and only returns declared fields', async () => {
    setDatabase(seedDatabase('default'));
    const result = (await invokeFixture('threads_action', {
      gestureId: null,
      targets: [{ accountId: 'acc-a', threadId: 'blue-01' }],
      action: { kind: 'archive' },
    })) as { gestureId: string; operations: unknown[] };
    expect(JSON.stringify(Object.keys(result).sort())).toBe(JSON.stringify(['gestureId', 'operations']));

    const inbox = (await invokeFixture('threads_query', {
      query: { accountIds: ['acc-a'], view: { kind: 'inbox' }, limit: 500 },
    })) as { rows: { id: string }[] };
    expect(inbox.rows.some((r) => r.id === 'blue-01')).toBe(false);

    const archive = (await invokeFixture('threads_query', {
      query: { accountIds: ['acc-a'], view: { kind: 'archive' }, limit: 500 },
    })) as { rows: { id: string }[] };
    expect(archive.rows.some((r) => r.id === 'blue-01')).toBe(true);

    await invokeFixture('action_undo', { gestureId: result.gestureId });
    const restored = (await invokeFixture('threads_query', {
      query: { accountIds: ['acc-a'], view: { kind: 'inbox' }, limit: 500 },
    })) as { rows: { id: string }[] };
    expect(restored.rows.some((r) => r.id === 'blue-01')).toBe(true);
  });

  it('rejects a cursor issued for a different query instead of silently reusing it', async () => {
    setDatabase(seedDatabase('default'));
    const page = (await invokeFixture('threads_query', {
      query: { accountIds: ['acc-a', 'acc-b'], view: { kind: 'inbox' }, limit: 100 },
    })) as { nextCursor?: string };
    await expect(
      invokeFixture('threads_query', {
        query: { accountIds: ['acc-a'], view: { kind: 'inbox' }, limit: 100, cursor: page.nextCursor },
      }),
    ).rejects.toThrow(/cursor_/);
  });
});
