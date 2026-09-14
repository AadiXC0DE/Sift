import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { DraftSaveQueue, newDraftId, type DraftSnapshot } from './draftQueue';
import type { Draft } from '../../app/ipc/types';

function snapshot(overrides: Partial<DraftSnapshot> = {}): DraftSnapshot {
  return {
    accountId: 'a1',
    mode: 'new',
    toJson: [],
    ccJson: [],
    bccJson: [],
    subject: '',
    bodyHtml: '',
    attachmentsJson: [],
    ...overrides,
  };
}

/** Storage that echoes what it stored, with a controllable delay. */
function storage() {
  const calls: Draft[] = [];
  let release: (() => void) | undefined;
  const gate = {
    hold: () => {
      release = undefined;
      // The next save waits until `let go` is called.
      gate.promise = new Promise<void>((r) => {
        release = r;
      });
    },
    letGo: () => release?.(),
    promise: Promise.resolve(),
  };
  const save = vi.fn(async (d: Draft): Promise<Draft> => {
    calls.push(d);
    if (gate.promise) await gate.promise;
    return { ...d, savedRevision: d.revision, updatedAt: 1 };
  });
  return { calls, save, gate };
}

beforeEach(() => {
  vi.useRealTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe('P5.1 one draft identity', () => {
  it('allocates a uuid the composer can hold for its whole life', () => {
    const a = newDraftId();
    const b = newDraftId();
    expect(a).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
    expect(a).not.toBe(b);
  });

  it('sends the same localId for every save and never takes one from a response', async () => {
    const { calls, save } = storage();
    const q = new DraftSaveQueue({ localId: 'stable-id', build: () => snapshot(), save });

    q.change();
    await q.flush();
    q.change();
    await q.flush();

    expect(calls.map((c) => c.localId)).toEqual(['stable-id', 'stable-id']);
    expect(q.localId).toBe('stable-id');
  });

  it('continues from the stored revision when reopening a draft', async () => {
    const { calls, save } = storage();
    const stored: Draft = {
      ...snapshot({ subject: 'Old' }),
      localId: 'd1',
      revision: 7,
      state: 'editing',
    };
    const q = new DraftSaveQueue({
      localId: 'd1',
      initial: stored,
      build: () => snapshot({ subject: 'New' }),
      save,
    });

    q.change();
    await q.flush();

    expect(calls[0].revision).toBe(8);
    expect(q.acknowledgedRevision).toBe(8);
  });

  it('writes nothing when a fresh composer is opened and closed untouched', async () => {
    const { save } = storage();
    const q = new DraftSaveQueue({ localId: 'd2', build: () => snapshot(), save });

    expect(await q.flush()).toBeNull();
    expect(save).not.toHaveBeenCalled();
  });
});

describe('P5.1 every edit is persisted', () => {
  it('debounces a burst into one write carrying the latest full snapshot', async () => {
    vi.useFakeTimers();
    const { calls, save } = storage();
    let subject = '';
    const q = new DraftSaveQueue({
      localId: 'd3',
      debounceMs: 300,
      build: () => snapshot({ subject, toJson: [{ e: 'ada@x.com' }] }),
      save,
    });

    for (const s of ['H', 'He', 'Hel', 'Hell', 'Hello']) {
      subject = s;
      q.change();
      vi.advanceTimersByTime(50);
    }
    expect(save).not.toHaveBeenCalled(); // still inside the quiet period

    await vi.advanceTimersByTimeAsync(300);

    expect(calls).toHaveLength(1);
    expect(calls[0].subject).toBe('Hello');
    expect(calls[0].toJson).toEqual([{ e: 'ada@x.com' }]);
    expect(calls[0].revision).toBe(5);
  });

  it('serializes writes: a slow save is never overlapped and the newest wins', async () => {
    const { calls, save, gate } = storage();
    let subject = 'first';
    const q = new DraftSaveQueue({ localId: 'd4', debounceMs: 0, build: () => snapshot({ subject }), save });

    gate.hold();
    q.change();
    const first = q.flush();
    await Promise.resolve();
    await Promise.resolve();
    expect(calls).toHaveLength(1);

    // An edit lands while the first write is still in flight.
    subject = 'second';
    q.change();
    const second = q.flush();
    await Promise.resolve();
    expect(calls).toHaveLength(1); // no second concurrent write

    gate.letGo();
    await first;
    await second;

    expect(calls.map((c) => c.subject)).toEqual(['first', 'second']);
    expect(calls[1].revision).toBeGreaterThan(calls[0].revision);
  });

  it('a lagging response for an older revision cannot roll the draft back', async () => {
    const stale: Draft = { ...snapshot({ subject: 'stale' }), localId: 'd5', revision: 1, state: 'editing' };
    const fresh: Draft = { ...snapshot({ subject: 'fresh' }), localId: 'd5', revision: 4, state: 'editing' };
    const save = vi.fn().mockResolvedValueOnce(fresh).mockResolvedValueOnce(stale);
    const q = new DraftSaveQueue({ localId: 'd5', debounceMs: 0, build: () => snapshot(), save });

    q.change();
    await q.flush();
    q.change();
    const res = await q.flush();

    expect(q.acknowledgedRevision).toBe(4);
    expect(res?.revision).toBe(4);
    expect(res?.subject).toBe('fresh');
    // The next revision is still ahead of what storage acknowledged.
    q.change();
    await q.flush();
    expect(save.mock.calls.at(-1)?.[0].revision).toBeGreaterThan(4);
  });

  it('reports a storage failure, keeps the snapshot, and retries on request', async () => {
    const statuses: string[] = [];
    const save = vi
      .fn()
      .mockRejectedValueOnce(new Error('disk full'))
      .mockImplementation(async (d: Draft) => ({ ...d, state: 'editing' as const }));
    const q = new DraftSaveQueue({
      localId: 'd6',
      debounceMs: 0,
      build: () => snapshot({ subject: 'kept' }),
      save,
      onStatus: (s) => statuses.push(s),
    });

    q.change();
    expect(await q.flush()).toBeNull();
    expect(q.currentStatus).toBe('error');
    expect(statuses).toContain('error');

    expect((await q.retry())?.subject).toBe('kept');
    expect(q.currentStatus).toBe('saved');
    expect(save.mock.calls.at(-1)?.[0].revision).toBeGreaterThanOrEqual(1);
  });
});
