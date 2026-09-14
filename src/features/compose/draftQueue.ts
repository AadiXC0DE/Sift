import type { Draft } from '../../app/ipc/types';

/**
 * The fields the composer owns. Everything the storage layer answers for
 * (`revision`, `savedRevision`, `remoteRevision`, `state`, `updatedAt`) is
 * filled in by the queue or the backend, never by the editor.
 */
export type DraftSnapshot = Omit<
  Draft,
  'localId' | 'revision' | 'savedRevision' | 'remoteRevision' | 'state' | 'updatedAt'
>;

export type SaveStatus = 'idle' | 'saving' | 'saved' | 'error';

/**
 * A new composer's stable draft identity. Allocated when the composer opens and
 * never again, so an asynchronous save cannot race a second id into existence.
 */
export function newDraftId(): string {
  const c = globalThis.crypto;
  if (c && typeof c.randomUUID === 'function') return c.randomUUID();
  const bytes = new Uint8Array(16);
  c?.getRandomValues(bytes);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export interface DraftSaveQueueOptions {
  /** Allocated once when the composer opens and never regenerated (P5.1). */
  localId: string;
  /**
   * A draft already in storage that this composer is editing. Its revision is
   * the starting point so every later write stays monotonically newer.
   */
  initial?: Draft;
  /** The newest complete snapshot, read at write time so a save can never post stale fields. */
  build: () => DraftSnapshot;
  save: (draft: Draft) => Promise<Draft>;
  /** Quiet period after the last edit before a write starts. */
  debounceMs?: number;
  onStatus?: (status: SaveStatus) => void;
}

/**
 * Every-edit draft persistence (P5.1).
 *
 * - One stable draft id for the life of the composer.
 * - `change()` bumps a monotonic revision for anything the user touched.
 * - Writes are serialized: at most one `save` is in flight, and the snapshot is
 *   read when that write starts, so the newest full state always wins.
 * - `savedRevision` only ever moves forward, so a slow response that lands after
 *   a newer one can never roll the draft back.
 * - `flush()` resolves with the draft that storage acknowledged for the newest
 *   revision, or null when storage failed — the composer stays open on null.
 */
export class DraftSaveQueue {
  readonly localId: string;
  private readonly build: () => DraftSnapshot;
  private readonly save: (draft: Draft) => Promise<Draft>;
  private readonly debounceMs: number;
  private readonly onStatus?: (status: SaveStatus) => void;

  private revision = 0;
  private savedRevision = 0;
  private lastSaved: Draft | null = null;
  private status: SaveStatus = 'idle';
  private timer: ReturnType<typeof setTimeout> | undefined;
  private tail: Promise<Draft | null> = Promise.resolve(null);
  private disposed = false;

  constructor(opts: DraftSaveQueueOptions) {
    this.localId = opts.localId;
    this.build = opts.build;
    this.save = opts.save;
    this.debounceMs = opts.debounceMs ?? 300;
    this.onStatus = opts.onStatus;
    if (opts.initial) {
      this.revision = opts.initial.revision;
      this.savedRevision = opts.initial.revision;
      this.lastSaved = opts.initial;
      this.status = 'saved';
    }
  }

  /** The revision the editor is currently on. */
  get currentRevision(): number {
    return this.revision;
  }

  /** Highest revision storage acknowledged. */
  get acknowledgedRevision(): number {
    return this.savedRevision;
  }

  get currentStatus(): SaveStatus {
    return this.status;
  }

  /** The last acknowledged draft, if any. */
  get saved(): Draft | null {
    return this.lastSaved;
  }

  /** Record one edit (recipients, subject, body, attachment, From, reply target). */
  change(): number {
    if (this.disposed) return this.revision;
    // Never below what storage already acknowledged: a conflict resolution that
    // stored a higher revision must not be followed by a regression.
    this.revision = Math.max(this.revision + 1, this.savedRevision + 1);
    this.schedule();
    return this.revision;
  }

  private setStatus(next: SaveStatus): void {
    if (this.status === next) return;
    this.status = next;
    this.onStatus?.(next);
  }

  private schedule(): void {
    if (this.timer !== undefined) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.flush();
    }, this.debounceMs);
  }

  /**
   * Write the newest snapshot and resolve with what storage acknowledged.
   * Safe to call at any time — close, send and Escape all funnel here.
   */
  flush(): Promise<Draft | null> {
    if (this.timer !== undefined) {
      clearTimeout(this.timer);
      this.timer = undefined;
    }
    if (this.disposed) return Promise.resolve(this.lastSaved);
    const first = this.enqueue();
    return first.then(async (res) => {
      // Storage refused this snapshot: report it instead of silently retrying,
      // so the caller can keep the composer open with a retry affordance.
      if (res === null) return null;
      // An edit landed while that write was running: it must be persisted too,
      // and the caller must see the draft for the revision it asked about.
      if (this.disposed || this.revision <= this.savedRevision) return res;
      return this.enqueue();
    });
  }

  /** Explicit retry from the "Couldn't save — Retry" affordance. */
  retry(): Promise<Draft | null> {
    return this.flush();
  }

  /**
   * Adopt a draft storage just returned for this id (P6.2 cancel/reopen). The
   * cancelled send is not an edit, so the revision storage acknowledged stays
   * the starting point for the next change — never a stale one that would make
   * the following write look like a conflict.
   */
  adopt(draft: Draft): void {
    if (draft.revision > this.savedRevision) this.savedRevision = draft.revision;
    this.revision = Math.max(this.revision, this.savedRevision);
    this.lastSaved = draft;
    this.setStatus('saved');
  }

  /** Stop scheduling writes. An in-flight write is left to finish. */
  dispose(): void {
    if (this.timer !== undefined) {
      clearTimeout(this.timer);
      this.timer = undefined;
    }
    this.disposed = true;
  }

  private enqueue(): Promise<Draft | null> {
    const job = this.tail.then(() => this.write());
    // The chain must survive a rejected write without unhandled rejections.
    this.tail = job.catch(() => null);
    return job;
  }

  /** One serialized write. Returns null when storage refused the snapshot. */
  private async write(): Promise<Draft | null> {
    if (this.disposed) return this.lastSaved;
    // Everything up to this revision is already durable: nothing to write.
    if (this.revision <= this.savedRevision) return this.lastSaved;
    const revision = this.revision;
    const draft: Draft = {
      ...this.build(),
      localId: this.localId,
      revision,
      state: this.lastSaved?.state ?? 'editing',
    };
    this.setStatus('saving');
    try {
      const res = await this.save(draft);
      const acked = typeof res?.revision === 'number' ? res.revision : revision;
      // Monotonic acknowledgement: a lagging response for an older revision is
      // never allowed to become the current draft.
      if (res && acked >= this.savedRevision) {
        this.savedRevision = acked;
        this.lastSaved = { ...res, revision: acked };
      }
      this.setStatus(this.revision > this.savedRevision ? 'saving' : 'saved');
      return this.lastSaved;
    } catch {
      // Storage failure is a storage failure: keep the snapshot for a retry and
      // never pretend it is an offline condition (P5.1 / SEND-01).
      this.setStatus('error');
      return null;
    }
  }
}
