/**
 * Deterministic, stateful fixture backend for the browser E2E suite.
 *
 * This module is the mock boundary: it receives exactly the argument objects
 * `src/app/ipc/commands.ts` sends and returns exactly the serialized shapes the
 * Rust side returns. It is only reachable through the test-only Vite mode
 * (`e2e/vite.e2e.config.ts` aliases the Tauri entry points here), so production
 * bundles can never include it.
 *
 * Two properties make the suite meaningful rather than decorative:
 *
 * - mutations are persisted to `localStorage`, so a reload observes the *saved*
 *   state and an action that only changed component state fails the test;
 * - list queries honour an opaque cursor carrying the full
 *   `(lastMessageAt, accountId, id)` sort tuple, so a mock that ignored the
 *   cursor would return page one again and the paging tests would see
 *   duplicates.
 */
import type {
  Account,
  AttachmentRef,
  AttachmentRefKey,
  ConnectivityState,
  Draft,
  DraftPage,
  Label,
  MessageBody,
  MessageMeta,
  SaveAllResult,
  Settings,
  StorageUsage,
  ThreadAction,
  ThreadDetail,
  ThreadRow,
  ThreadsPage,
  ThreadsQuery,
  View,
} from '../../src/app/ipc/types';
import { emit } from './bus';
import {
  ACCOUNT_A,
  ACCOUNT_B,
  FIXED_NOW,
  FIXTURE_DB_KEY,
  type FixtureDb,
  type FixtureDraft,
  type FixtureMessage,
  type FixtureNotificationState,
  type FixtureOutboxOp,
  type FixtureReminder,
  type FixtureRule,
  type FixtureThread,
  seedDatabase,
  type Scenario,
} from './dataset';
// The same wall-clock helpers the app uses, so the fixture's scheduled local
// time and zone are produced exactly the way a real queue produces them (P8.1).
import { localWallTime, timezoneName } from '../../src/features/mail-utilities/time';

export type Args = Record<string, unknown>;

interface CursorTuple {
  ts: number;
  accountId: string;
  id: string;
}

interface CursorPayload {
  k: CursorTuple;
  v: string;
  a: string[];
}

interface UndoSnapshot {
  threads: { accountId: string; id: string; labelIds: string[]; isStarred: boolean; snoozedUntil?: number }[];
  messages: { accountId: string; id: string; labelIds: string[]; isUnread: boolean; isStarred: boolean }[];
}

const MAX_LIMIT = 500;

let db: FixtureDb;
let undoGroups: Record<string, UndoSnapshot> = {};
const callLog: { cmd: string; at: number; args?: Args }[] = [];
const unimplemented: string[] = [];
const saveAsCalls: { attachmentId: string; filename: string }[] = [];
/** Destination paths the fixture has written, so collisions are observable. */
const savedPaths: string[] = [];
/** Attachment ids that Save All is asked to fail, to exercise the retry path. */
const saveAllFailures = new Set<string>();
const openCalls: string[] = [];
const openUrlCalls: string[] = [];
const delays: Record<string, (args: Args) => number> = {};
let failAppPassword = false;
/** Remaining `drafts_upsert` calls the fixture must reject (P5.1 storage failure). */
let failDraftSaves = 0;
/**
 * Remaining `message_body` rejections per `${accountId}:${messageId}` (P9.2):
 * a provider can fail the same message twice before it answers.
 */
const bodyFailures: Record<string, number> = {};
/**
 * Every body the app actually fetched over IPC, in order. Bodies are cached in
 * the app, so this is the evidence that a cache hit costs nothing (P9.2).
 */
const bodyFetches: { accountId: string; messageId: string; at: number }[] = [];

/**
 * Connectivity (P4.6), mirroring `connectivity.rs`: the host hint is only a
 * hint, and each account keeps its own evidence until an operation succeeds.
 */
interface AccountHealth {
  failure: 'connectivity' | 'auth' | null;
  lastOkAt: number | null;
  lastError: string | null;
  since: number;
}
let network = true;
let health: Record<string, AccountHealth> = {};

function healthOf(accountId: string): AccountHealth {
  return (health[accountId] ??= {
    failure: null,
    lastOkAt: database().accounts.find((a) => a.id === accountId)?.last_sync_at ?? null,
    lastError: null,
    since: FIXED_NOW,
  });
}

function connectivityRow(accountId: string): ConnectivityState {
  const h = healthOf(accountId);
  const state: ConnectivityState['state'] = !network
    ? 'offline'
    : h.failure === 'auth'
      ? 'reauth_required'
      : h.failure === 'connectivity'
        ? 'offline'
        : 'online';
  return {
    accountId,
    state,
    lastOkAt: h.lastOkAt,
    lastError: h.failure ? h.lastError : null,
    since: h.since,
  };
}

function emitConnectivity(accountId: string): void {
  emit('connectivity:state', connectivityRow(accountId));
}

function failAccount(accountId: string, failure: 'connectivity' | 'auth', message: string): void {
  const h = healthOf(accountId);
  h.failure = failure;
  h.lastError = message;
  h.since = FIXED_NOW;
  emitConnectivity(accountId);
}

/** IPC boundary for operations: the stored payload never leaves the fixture. */
function outboxSummary(op: FixtureOutboxOp) {
  return {
    opId: op.op_id,
    accountId: op.accountId,
    kind: op.kind,
    state: op.state,
    action: op.action,
    recipientSummary: op.recipientSummary || null,
    subject: op.subject || null,
    scheduledAt: op.scheduledAt,
    scheduledLocalTime: op.scheduledLocalTime ?? null,
    scheduledTimezone: op.scheduledTimezone ?? null,
    canSendNow: op.state === 'pending',
    retryAt: op.retryAt,
    createdAt: op.createdAt,
    errorCode: op.errorCode,
    errorMessage: op.errorMessage,
    draftId: op.draftId,
    revision: op.revision,
    requiresDuplicateAck: op.requiresDuplicateAck,
  };
}

/**
 * A gesture's operation summary. Label work is drained immediately, so the
 * fixture never parks it in the outbox: these summaries exist for the response
 * the toast reads, in the same shape `outbox_list` returns.
 */
function gestureOp(accountId: string, action: string) {
  return {
    opId: database().nextOpId++,
    accountId,
    kind: 'label',
    state: 'pending' as const,
    action,
    recipientSummary: null,
    subject: null,
    scheduledAt: null,
    retryAt: null,
    createdAt: FIXED_NOW,
    errorCode: null,
    errorMessage: null,
    draftId: null,
    revision: null,
    requiresDuplicateAck: false,
  };
}

function groupByLabel(ops: FixtureOutboxOp[]): Map<string, { label: string; count: number }> {
  const groups = new Map<string, { label: string; count: number }>();
  for (const op of ops) {
    const label = op.subject || op.action;
    const found = groups.get(label);
    if (found) found.count += 1;
    else groups.set(label, { label, count: 1 });
  }
  return groups;
}

function countOps(ops: FixtureOutboxOp[]) {
  const counts = { pending: 0, inflight: 0, uncertain: 0, done: 0, failed: 0, cancelled: 0 };
  for (const op of ops) counts[op.state] += 1;
  return counts;
}

function emitOutboxState(accountId: string): void {
  const ops = database().outbox.filter((o) => o.accountId === accountId);
  const counts = countOps(ops);
  emit('outbox:state', {
    account_id: accountId,
    pending: counts.pending,
    inflight: counts.inflight,
    failed: counts.failed,
    uncertain: counts.uncertain,
    // Aggregated like the backend's summary columns: a queue of 10,000 must
    // not arrive as 10,000 entries.
    summary: [...groupByLabel(ops.filter((o) => o.state === 'pending' || o.state === 'inflight'))],
  });
}

function scenarioFromLocation(): Scenario {
  try {
    const raw = new URLSearchParams(window.location.search).get('scenario');
    return raw === 'empty' ? 'empty' : 'default';
  } catch {
    return 'default';
  }
}

function persist(): void {
  try {
    window.localStorage.setItem(FIXTURE_DB_KEY, JSON.stringify(db));
  } catch {
    /* storage unavailable: the in-memory snapshot is still authoritative */
  }
}

function load(): FixtureDb {
  const scenario = scenarioFromLocation();
  try {
    const raw = window.localStorage.getItem(FIXTURE_DB_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as FixtureDb;
      if (parsed?.version === 1 && parsed.scenario === scenario) return parsed;
    }
  } catch {
    /* fall through to a fresh seed */
  }
  const fresh = seedDatabase(scenario);
  db = fresh;
  persist();
  return fresh;
}

export function database(): FixtureDb {
  if (!db) db = load();
  return db;
}

function sleep(ms: number): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  window.setTimeout(resolve, ms);
  return promise;
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

function viewMatches(t: FixtureThread, view: View): boolean {
  const has = (l: string) => t.labelIds.includes(l);
  // Trash and Junk are decided over every message of the conversation (the Rust
  // view reads `t.in_trash`/`t.in_spam`), so a conversation with one trashed
  // message and one inbox message is *not* in Trash — it is in All Mail.
  const msgs = messagesOf(t.accountId, t.id);
  const all = (l: string) => msgs.length > 0 && msgs.every((m) => m.labelIds.includes(l));
  switch (view.kind) {
    case 'inbox':
      return has('INBOX');
    case 'starred':
      return t.isStarred;
    case 'snoozed':
      return typeof t.snoozedUntil === 'number';
    case 'sent':
      return has('SENT');
    case 'drafts':
      return has('DRAFT');
    case 'archive':
      return !has('INBOX') && !has('TRASH') && !has('SPAM') && !has('DRAFT') && !has('SENT');
    case 'all_mail':
      // P3.6: membership is per message, exactly like
      // `db/threads.rs::view_where` — a thread is in All Mail when at least one
      // of its messages carries neither TRASH nor SPAM. It is a mailbox, never
      // the account scope: `accountIds` still filters the rows.
      return msgs.some((m) => !m.labelIds.includes('TRASH') && !m.labelIds.includes('SPAM'));
    case 'spam':
      return all('SPAM');
    case 'trash':
      return all('TRASH');
    case 'label':
      return has(view.labelId);
    case 'search': {
      const q = view.q.trim().toLowerCase();
      if (!q) return false;
      return q.split(/\s+/).every((term) => t.searchText.includes(term));
    }
    default:
      return false;
  }
}

function queryKey(q: ThreadsQuery): string {
  return JSON.stringify({
    accountIds: [...q.accountIds].sort(),
    view: q.view,
    u: !!q.unread_only,
    h: !!q.has_attachment,
  });
}

function tupleOf(t: FixtureThread): CursorTuple {
  return { ts: t.lastMessageAt, accountId: t.accountId, id: t.id };
}

function compareTuple(a: CursorTuple, b: CursorTuple): number {
  if (a.ts !== b.ts) return b.ts - a.ts;
  if (a.accountId !== b.accountId) return a.accountId < b.accountId ? 1 : -1;
  if (a.id !== b.id) return a.id < b.id ? 1 : -1;
  return 0;
}

function encodeCursor(t: FixtureThread, key: string, accountIds: string[]): string {
  const payload: CursorPayload = { k: tupleOf(t), v: key, a: [...accountIds].sort() };
  return window.btoa(JSON.stringify(payload));
}

function decodeCursor(cursor: string, key: string, accountIds: string[]): CursorTuple | null {
  try {
    const payload = JSON.parse(window.atob(cursor)) as CursorPayload;
    if (!payload?.k || typeof payload.k.ts !== 'number') throw new Error('cursor_shape');
    if (payload.v !== key) throw new Error('cursor_query_mismatch');
    if (JSON.stringify([...payload.a].sort()) !== JSON.stringify([...accountIds].sort())) {
      throw new Error('cursor_scope_mismatch');
    }
    return payload.k;
  } catch (e) {
    throw new Error(`cursor_invalid: ${(e as Error).message}`);
  }
}

function pageOf(query: ThreadsQuery, source: FixtureThread[]): ThreadsPage {
  const limit = Math.min(MAX_LIMIT, Math.max(1, Number(query.limit) || 100));
  const key = queryKey(query);
  const cursor = query.cursor ? decodeCursor(query.cursor, key, query.accountIds) : null;
  const matched = source
    .filter((t) => query.accountIds.includes(t.accountId))
    .filter((t) => viewMatches(t, query.view))
    .filter((t) => (query.unread_only ? t.unreadCount > 0 : true))
    .filter((t) => (query.has_attachment ? t.hasAttachments : true))
    .sort((a, b) => compareTuple(tupleOf(a), tupleOf(b)));
  const after = cursor ? matched.filter((t) => compareTuple(tupleOf(t), cursor) > 0) : matched;
  const rows = after.slice(0, limit);
  const last = rows[rows.length - 1];
  if (after.length > limit && last) {
    return {
      rows: rows.map(toThreadRow),
      nextCursor: encodeCursor(last, key, query.accountIds),
      generation: 1,
    };
  }
  return { rows: rows.map(toThreadRow), generation: 1 };
}

// ---------------------------------------------------------------------------
// Mutation helpers
// ---------------------------------------------------------------------------

function threadRows(): FixtureThread[] {
  return database().threads;
}

function findThread(accountId: string, id: string): FixtureThread | undefined {
  return threadRows().find((t) => t.accountId === accountId && t.id === id);
}

function messagesOf(accountId: string, threadId: string): FixtureMessage[] {
  return database().messages.filter((m) => m.accountId === accountId && m.threadId === threadId);
}

function recomputeThread(t: FixtureThread): void {
  const msgs = messagesOf(t.accountId, t.id);
  if (!msgs.length) return;
  t.unreadCount = msgs.filter((m) => m.isUnread && !m.isDraft).length;
  t.isStarred = msgs.some((m) => m.isStarred);
  t.hasAttachments = msgs.some((m) => m.hasAttachments);
  t.lastMessageAt = Math.max(...msgs.map((m) => m.internalDate));
  t.searchText =
    `${t.subject} ${t.snippet} ${msgs.map((m) => `${m.from.e} ${m.from.n ?? ''}`).join(' ')}`.toLowerCase();
}

function labelCounts(accountId: string): void {
  const rows = threadRows().filter((t) => t.accountId === accountId);
  for (const l of database().labels.filter((l) => l.account_id === accountId)) {
    l.total_count = rows.filter((t) => t.labelIds.includes(l.id)).length;
    if (l.id === 'INBOX' || l.id === 'SPAM') {
      l.unread_count = rows.filter((t) => t.labelIds.includes(l.id) && t.unreadCount > 0).length;
    }
  }
}

function afterMutation(accountIds: string[], threadIds: string[]): void {
  persist();
  for (const accountId of new Set(accountIds)) labelCounts(accountId);
  for (const accountId of new Set(accountIds)) {
    emit('store:labels', { account_id: accountId });
    emit('store:threads', { account_id: accountId, thread_ids: threadIds });
  }
}

/** IPC boundary for thread rows: the fixture-only search text never leaves. */
function toThreadRow(t: FixtureThread): ThreadRow {
  const row: ThreadRow = {
    accountId: t.accountId,
    id: t.id,
    subject: t.subject,
    snippet: t.snippet,
    participants: t.participants,
    lastMessageAt: t.lastMessageAt,
    messageCount: t.messageCount,
    unreadCount: t.unreadCount,
    isStarred: t.isStarred,
    hasAttachments: t.hasAttachments,
    labelIds: t.labelIds,
  };
  if (t.snoozedUntil !== undefined) row.snoozedUntil = t.snoozedUntil;
  if (t.serverOnly !== undefined) row.serverOnly = t.serverOnly;
  return row;
}

/** IPC boundary for drafts: the fixture stores exactly what the app sends. */
function toDraftDto(d: FixtureDraft): Draft {
  return {
    localId: d.localId,
    accountId: d.accountId,
    fromEmail: d.fromEmail,
    mode: d.mode,
    toJson: d.toJson,
    ccJson: d.ccJson,
    bccJson: d.bccJson,
    subject: d.subject,
    bodyHtml: d.bodyHtml,
    attachmentsJson: d.attachmentsJson,
    threadId: d.threadId,
    inReplyToMessageId: d.inReplyToMessageId,
    rfcMessageId: d.rfcMessageId,
    revision: d.revision,
    state: d.state,
    updatedAt: d.updatedAt,
  };
}

function stripMessage(m: FixtureMessage): MessageMeta {
  const meta: MessageMeta = {
    id: m.id,
    internalDate: m.internalDate,
    from: m.from,
    to: m.to,
    cc: m.cc,
    bcc: m.bcc,
    subject: m.subject,
    snippet: m.snippet,
    isUnread: m.isUnread,
    isStarred: m.isStarred,
    isDraft: m.isDraft,
    isSentByMe: m.isSentByMe,
    labelIds: m.labelIds,
    hasAttachments: m.hasAttachments,
    attachments: m.attachments,
    bodyState: m.bodyState,
  };
  if (m.replyTo !== undefined) meta.replyTo = m.replyTo;
  if (m.listUnsubscribe !== undefined) meta.listUnsubscribe = m.listUnsubscribe;
  return meta;
}

function applyAction(t: FixtureThread, action: ThreadAction['action']): void {
  const add = (l: string) => {
    if (!t.labelIds.includes(l)) t.labelIds.push(l);
  };
  const remove = (l: string) => {
    t.labelIds = t.labelIds.filter((x) => x !== l);
  };
  const msgs = messagesOf(t.accountId, t.id);
  switch (action.kind) {
    case 'archive':
      remove('INBOX');
      for (const m of msgs) m.labelIds = m.labelIds.filter((x) => x !== 'INBOX');
      break;
    case 'unarchive':
      add('INBOX');
      for (const m of msgs) if (!m.labelIds.includes('INBOX')) m.labelIds.push('INBOX');
      break;
    case 'trash':
      remove('INBOX');
      remove('SPAM');
      add('TRASH');
      for (const m of msgs) {
        m.labelIds = m.labelIds.filter((x) => x !== 'INBOX' && x !== 'SPAM');
        if (!m.labelIds.includes('TRASH')) m.labelIds.push('TRASH');
      }
      break;
    case 'untrash':
      remove('TRASH');
      add('INBOX');
      for (const m of msgs) {
        m.labelIds = m.labelIds.filter((x) => x !== 'TRASH');
        if (!m.labelIds.includes('INBOX')) m.labelIds.push('INBOX');
      }
      break;
    case 'spam':
      remove('INBOX');
      add('SPAM');
      for (const m of msgs) {
        m.labelIds = m.labelIds.filter((x) => x !== 'INBOX');
        if (!m.labelIds.includes('SPAM')) m.labelIds.push('SPAM');
      }
      break;
    case 'unspam':
      remove('SPAM');
      add('INBOX');
      for (const m of msgs) {
        m.labelIds = m.labelIds.filter((x) => x !== 'SPAM');
        if (!m.labelIds.includes('INBOX')) m.labelIds.push('INBOX');
      }
      break;
    case 'star':
      t.isStarred = action.on;
      if (action.on) add('STARRED');
      else remove('STARRED');
      for (const m of msgs) {
        m.isStarred = action.on;
        m.labelIds = action.on
          ? m.labelIds.includes('STARRED')
            ? m.labelIds
            : [...m.labelIds, 'STARRED']
          : m.labelIds.filter((x) => x !== 'STARRED');
      }
      break;
    case 'read':
      for (const m of msgs) {
        m.isUnread = !action.on;
        m.labelIds = action.on
          ? m.labelIds.filter((x) => x !== 'UNREAD')
          : m.labelIds.includes('UNREAD')
            ? m.labelIds
            : [...m.labelIds, 'UNREAD'];
      }
      t.unreadCount = action.on ? 0 : Math.max(1, msgs.length);
      if (action.on) remove('UNREAD');
      else add('UNREAD');
      break;
    case 'addLabel':
      add(action.labelId);
      for (const m of msgs) if (!m.labelIds.includes(action.labelId)) m.labelIds.push(action.labelId);
      break;
    case 'removeLabel':
      remove(action.labelId);
      for (const m of msgs) m.labelIds = m.labelIds.filter((x) => x !== action.labelId);
      break;
    case 'moveTo':
      t.labelIds = [action.labelId];
      for (const m of msgs) m.labelIds = [action.labelId];
      break;
    case 'deleteForever':
      db.threads = db.threads.filter((x) => !(x.accountId === t.accountId && x.id === t.id));
      db.messages = db.messages.filter((m) => !(m.accountId === t.accountId && m.threadId === t.id));
      break;
    default:
      throw new Error('unsupported action');
  }
  if (action.kind !== 'deleteForever') recomputeThread(t);
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

function attachmentMeta(
  m: FixtureMessage,
  id: string,
): { filename?: string | null; mime: string; size: number } {
  const a = m.attachments.find((x) => x.id === id);
  if (!a) throw new Error('attachment_part_missing');
  return { filename: a.filename, mime: a.mime, size: a.size };
}

/** Mirrors `attachments::naming::basename` for the types the seed uses. */
function attachmentBasename(a: { filename?: string | null; mime: string; id: string }): string {
  const named = a.filename?.trim();
  if (named) return named;
  const short = a.id.replace(/[^A-Za-z0-9]/g, '').slice(0, 8) || '0';
  return `attachment-${short}.bin`;
}

/** `name (2).ext` collision handling, mirroring `naming::unique_destination`. */
function uniqueDestination(name: string): string {
  const taken = new Set(savedPaths);
  if (!taken.has(name)) return name;
  const dot = name.lastIndexOf('.');
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : '';
  for (let n = 2; n < 10_000; n++) {
    const candidate = `${stem} (${n})${ext}`;
    if (!taken.has(candidate)) return candidate;
  }
  return name;
}

function threadDetail(accountId: string, threadId: string): ThreadDetail {
  const t = findThread(accountId, threadId);
  if (!t) throw new Error('thread_not_found');
  return {
    accountId: t.accountId,
    id: t.id,
    subject: t.subject,
    labelIds: [...t.labelIds],
    messages: messagesOf(accountId, threadId).map(stripMessage),
  };
}

function usageSnapshot(): StorageUsage {
  const bodies = database().messages.reduce((sum, m) => sum + m.html.length + m.text.length, 0);
  const attachments = database().messages.reduce(
    (sum, m) => sum + m.attachments.reduce((s, a) => s + a.size, 0),
    0,
  );
  const metadata = JSON.stringify(database().threads).length;
  const draftCache = database().drafts.reduce(
    (s, d) => s + d.bodyHtml.length + JSON.stringify(d.attachmentsJson).length,
    0,
  );
  const attachmentsCategory = {
    bytes: attachments,
    items: database().messages.reduce((s, m) => s + m.attachments.length, 0),
  };
  return {
    metadata: { bytes: metadata, items: database().threads.length },
    bodies: { bytes: bodies, items: database().messages.length },
    attachments: attachmentsCategory,
    draftCache: { bytes: draftCache, items: database().drafts.length },
    pinnedBytes: 0,
    attachmentCacheLimitBytes: attachmentCapBytes,
    totalBytes: metadata + bodies + attachments + draftCache,
    computedAt: FIXED_NOW,
  };
}

// ---------------------------------------------------------------------------
// P8 local state helpers (Send Later, reminders, rules, notifications)
// ---------------------------------------------------------------------------

/**
 * The fixture's clock. The page pins `Date.now` at seed time; `advanceClock`
 * moves it so "sleep across the due time" and "quit and relaunch" are real
 * transitions rather than assumptions (P8.1/P8.2).
 */
let clockSkew = 0;
function now(): number {
  return FIXED_NOW + clockSkew;
}

function setClock(skew: number): void {
  clockSkew = skew;
  // `index.e2e.html` owns the pinned clock. Moving it has to move every reader
  // of "now" — `Date.now()` and `new Date()` alike — or the app computes a
  // present the fixture disagrees with and no deadline is ever crossed.
  window.__SIFT_CLOCK__.now = FIXED_NOW + clockSkew;
}

/** The configured attachment-cache cap (P10.4); settable through the UI. */
let attachmentCapBytes = 2 * 1024 * 1024 * 1024;

function outboxOp(opId: number): FixtureOutboxOp {
  const op = database().outbox.find((o) => o.op_id === opId);
  if (!op) throw new Error('op_not_found');
  return op;
}

function scheduleHandle(op: FixtureOutboxOp) {
  return {
    opId: op.op_id,
    notBefore: op.notBefore,
    scheduledAt: op.scheduledAt,
    scheduledLocalTime: op.scheduledLocalTime,
    scheduledTimezone: op.scheduledTimezone,
  };
}

/** What `reminders_list` returns: a reminder plus the conversation it names. */
export interface ReminderDto {
  accountId: string;
  threadId: string;
  remindAt: number;
  completedAt: number | null;
  deliveredAt: number | null;
  state: FixtureReminder['state'];
  subject: string | null;
  fromName: string | null;
  unread: boolean;
}

function reminderRow(r: FixtureReminder): ReminderDto {
  const thread = findThread(r.accountId, r.threadId);
  const last = messagesOf(r.accountId, r.threadId).at(-1);
  return {
    accountId: r.accountId,
    threadId: r.threadId,
    remindAt: r.remindAt,
    completedAt: r.completedAt,
    deliveredAt: r.deliveredAt,
    state: r.state,
    subject: thread?.subject ?? null,
    fromName: last?.from.n ?? last?.from.e ?? null,
    unread: (thread?.unreadCount ?? 0) > 0,
  };
}

function reminderRows(accountIds: string[], includeCompleted = false) {
  return database()
    .reminders.filter((r) => accountIds.includes(r.accountId))
    .filter((r) => includeCompleted || r.completedAt == null)
    .map(reminderRow);
}

function labelOf(accountId: string, labelId: string): Label {
  const label = database().labels.find((l) => l.account_id === accountId && l.id === labelId);
  if (!label) throw new Error('label_not_found');
  return label;
}

function trashThreads(accountIds: string[]): FixtureThread[] {
  return database().threads.filter((t) => accountIds.includes(t.accountId) && t.labelIds.includes('TRASH'));
}

function ownedMessage(accountId: string, messageId: string): FixtureMessage {
  const message = database().messages.find((m) => m.accountId === accountId && m.id === messageId);
  if (!message) throw new Error('message_not_found');
  return message;
}

/** Whether one rule's conditions match a thread's own metadata (P8.3). */
function ruleMatches(rule: FixtureRule, t: FixtureThread): boolean {
  const probes: Record<string, string[]> = {
    sender: t.participants.map((p) => `${p.e} ${p.n ?? ''}`.toLowerCase()),
    recipient: messagesOf(t.accountId, t.id)
      .flatMap((m) => [...m.to, ...m.cc])
      .map((a) => a.e.toLowerCase()),
    subject: [t.subject.toLowerCase()],
    hasAttachment: [String(t.hasAttachments)],
  };
  const test = (c: FixtureRule['conditions'][number]): boolean => {
    if (c.field === 'hasAttachment') return t.hasAttachments;
    const haystack = probes[c.field] ?? [];
    const needle = c.value.toLowerCase().trim();
    if (!needle) return false;
    if (c.op === 'domain') return haystack.some((h) => h.includes(`@${needle.replace(/^@/, '')}`));
    if (c.op === 'is') return haystack.some((h) => h.split(/\s+/).includes(needle) || h === needle);
    return haystack.some((h) => h.includes(needle));
  };
  if (!rule.conditions.length) return false;
  return rule.match === 'all' ? rule.conditions.every(test) : rule.conditions.some(test);
}

/** Applies a rule's actions; returns false when nothing changed. */
function applyRuleActions(rule: FixtureRule, t: FixtureThread): boolean {
  let changed = false;
  for (const action of rule.actions) {
    const before = t.labelIds.join(',');
    if (action.kind === 'addLabel' && action.labelId)
      applyAction(t, { kind: 'addLabel', labelId: action.labelId });
    if (action.kind === 'archive') applyAction(t, { kind: 'archive' });
    if (action.kind === 'junk') applyAction(t, { kind: 'spam' });
    if (action.kind === 'markRead') {
      for (const m of messagesOf(t.accountId, t.id)) m.isUnread = false;
      applyAction(t, { kind: 'read', on: true });
    }
    if (action.kind === 'star') {
      const msgs = messagesOf(t.accountId, t.id);
      const standing = msgs.filter((m) => m.isStarred).at(-1) ?? msgs.at(-1);
      if (standing) standing.isStarred = true;
      applyAction(t, { kind: 'star', on: true });
    }
    if (t.labelIds.join(',') !== before) changed = true;
  }
  // A rule's own label change must not re-trigger it on the next evaluation.
  return changed;
}

function toPreviewRow(t: FixtureThread) {
  const last = messagesOf(t.accountId, t.id).at(-1);
  return {
    threadId: t.id,
    subject: t.subject,
    fromName: last?.from.n ?? null,
    fromEmail: last?.from.e ?? '',
    wouldJunk: t.labelIds.includes('SPAM'),
  };
}

/** The unread-Inbox-thread count the runtime publishes as the dock badge. */
function unreadInboxThreads(): number {
  return database().threads.filter((t) => t.labelIds.includes('INBOX') && t.unreadCount > 0).length;
}

function emitBadge(): void {
  emit('badge:update', { count: unreadInboxThreads() });
}

/**
 * Whether one thread is eligible for a notification under the configured
 * filter (P8.4). `vip` needs the sender to be on the account's VIP list; `off`
 * and a disabled account are silent.
 */
function notificationAllowed(accountId: string, t: FixtureThread): boolean {
  const cfg = database().notifications;
  if (!cfg.enabled || cfg.filter === 'off') return false;
  if (!cfg.accountIds.includes(accountId)) return false;
  if (cfg.filter === 'vip') {
    const vips = database().vips[accountId] ?? [];
    return t.participants.some((p) => vips.includes(p.e.toLowerCase()));
  }
  return t.labelIds.includes('INBOX');
}

/** Every notification the runtime emitted, in order — the "one message, one notification" evidence. */
const notifyLog: { accountId: string; threadId: string; subject: string | null; hidden: boolean }[] = [];
let lastNotified: { accountId: string; threadId: string } | null = null;

function notifyThread(accountId: string, t: FixtureThread): void {
  if (!notificationAllowed(accountId, t)) return;
  const cfg = database().notifications;
  const entry = {
    accountId,
    threadId: t.id,
    subject: cfg.hideSubject ? null : t.subject,
    hidden: cfg.hideSubject,
  };
  notifyLog.push(entry);
  lastNotified = { accountId, threadId: t.id };
  emit('notify:new', {
    accountId,
    threadId: t.id,
    title: t.participants[0]?.n ?? t.participants[0]?.e ?? 'New mail',
    subject: entry.subject,
    body: cfg.hideSubject ? 'New message' : t.snippet,
    hidden: cfg.hideSubject,
  });
}

const handlers: Record<string, (args: Args) => unknown> = {
  accounts_list: () => database().accounts,
  accounts_add_google: (args) => {
    const loginHint = typeof args.loginHint === 'string' ? args.loginHint : 'demo@example.test';
    const existing = database().accounts.find((a) => a.email === loginHint);
    if (existing) return existing;
    const a: Account = {
      id: `acc-${database().accounts.length + 1}`,
      provider: 'gmail',
      email: loginHint,
      display_name: loginHint.split('@')[0],
      avatar_url: null,
      color: 'teal',
      auth_kind: 'oauth',
      history_id: null,
      sync_state: 'partial',
      last_sync_at: FIXED_NOW,
      created_at: FIXED_NOW,
      sort_order: database().accounts.length,
      signature_html: null,
    };
    database().accounts.push(a);
    persist();
    emit('store:labels', { account_id: a.id });
    return a;
  },
  accounts_add_app_password: (args) => {
    if (failAppPassword) throw new Error('invalid_credentials');
    const email = String(args.email ?? 'you@gmail.com');
    const existing = database().accounts.find((a) => a.email === email);
    if (existing) return existing;
    const a: Account = {
      id: `acc-${database().accounts.length + 1}`,
      provider: 'gmail',
      email,
      display_name: email.split('@')[0],
      avatar_url: null,
      color: 'teal',
      auth_kind: 'app_password',
      history_id: null,
      sync_state: 'partial',
      last_sync_at: FIXED_NOW,
      created_at: FIXED_NOW,
      sort_order: database().accounts.length,
      signature_html: null,
    };
    database().accounts.push(a);
    persist();
    emit('store:labels', { account_id: a.id });
    return a;
  },
  accounts_probe_email: () => null,
  accounts_update_app_password: (args) => {
    const a = database().accounts.find((x) => x.id === args.id);
    if (!a) throw new Error('account_not_found');
    return a;
  },
  accounts_update: (args) => {
    const id = args.id as string;
    const a = database().accounts.find((x) => x.id === id);
    if (!a) throw new Error('account_not_found');
    Object.assign(a, args);
    persist();
    return a;
  },
  accounts_remove: (args) => {
    const id = args.id as string;
    db.accounts = db.accounts.filter((a) => a.id !== id);
    db.threads = db.threads.filter((t) => t.accountId !== id);
    db.messages = db.messages.filter((m) => m.accountId !== id);
    db.labels = db.labels.filter((l) => l.account_id !== id);
    delete health[id];
    persist();
    emit('store:threads', { account_id: id, thread_ids: [] });
    emit('store:labels', { account_id: id });
    return null;
  },
  system_info: () => ({ version: 'e2e-fixture', oauth_available: false, demo: false }),
  sync_now: (args) => {
    const accountId = typeof args.accountId === 'string' ? args.accountId : undefined;
    if (!accountId) return null;
    const h = healthOf(accountId);
    if (network) {
      // A successful provider operation clears THIS account's error only.
      h.failure = null;
      h.lastError = null;
      h.lastOkAt = FIXED_NOW;
    } else {
      h.failure = 'connectivity';
      h.lastError = 'No network connection';
    }
    emitConnectivity(accountId);
    return null;
  },
  connectivity_state: () => database().accounts.map((a) => connectivityRow(a.id)),
  app_network_hint: (args) => {
    network = args.online !== false;
    for (const a of database().accounts) emitConnectivity(a.id);
    return null;
  },
  sync_status: () => database().sync,
  labels_list: (args) => database().labels.filter((l) => l.account_id === args.accountId),
  labels_create: (args) => {
    const account_id = String(args.accountId);
    const name = String(args.name ?? 'New label');
    const l: Label = {
      account_id,
      id: `Label_${account_id}_${name.replace(/\W+/g, '_')}`,
      name,
      kind: 'user',
      color_bg: null,
      color_fg: null,
      visible: true,
      unread_count: 0,
      total_count: 0,
      sort_order: 99,
      depth: 0,
    };
    database().labels.push(l);
    persist();
    emit('store:labels', { account_id });
    return l;
  },
  threads_query: (args) => pageOf(args.query as ThreadsQuery, threadRows()),
  thread_get: (args) => threadDetail(String(args.accountId), String(args.threadId)),
  message_body: (args) => {
    const id = String(args.messageId);
    const accountId = typeof args.accountId === 'string' ? args.accountId : '';
    // The account is part of the lookup: two accounts can hold the same
    // provider message id, and a body must never leak across them (P9.2).
    const owned = accountId
      ? database().messages.find((x) => x.accountId === accountId && x.id === id)
      : database().messages.find((x) => x.id === id);
    if (!owned) throw new Error('message_not_found');
    const key = `${owned.accountId}:${owned.id}`;
    if ((bodyFailures[key] ?? 0) > 0) {
      bodyFailures[key] -= 1;
      throw new Error('provider offline');
    }
    bodyFetches.push({ accountId: owned.accountId, messageId: owned.id, at: Date.now() });
    const body: MessageBody = {
      messageId: owned.id,
      state: 'ready',
      text: owned.text,
      remoteImageCount: 0,
      trackerCount: 0,
      darkSafe: true,
      remoteImagesAllowed: true,
    };
    // A message with no HTML part must reach the reader without one, so a
    // plain-only body renders as text rather than an empty frame (P9.2).
    if (owned.html !== '') body.html = owned.html;
    return body;
  },
  message_raw_source: (args) => {
    const id = String(args.messageId);
    const m = database().messages.find((x) => x.id === id);
    if (!m) throw new Error('message_not_found');
    return `From: ${m.from.e}\r\nSubject: ${m.subject}\r\n\r\n${m.text}\r\n`;
  },
  threads_action: (args) => {
    const targets = (args.targets as { accountId: string; threadId: string }[]) ?? [];
    const action = args.action as ThreadAction['action'];
    const gestureId =
      typeof args.gestureId === 'string' ? args.gestureId : `gesture-${database().nextOpId++}`;
    const snapshot: UndoSnapshot = { threads: [], messages: [] };
    const touchedByAccount = new Map<string, string[]>();
    for (const target of targets) {
      const t = findThread(target.accountId, target.threadId);
      if (!t) continue;
      const touched = touchedByAccount.get(target.accountId) ?? [];
      touched.push(t.id);
      touchedByAccount.set(target.accountId, touched);
      snapshot.threads.push({
        accountId: t.accountId,
        id: t.id,
        labelIds: [...t.labelIds],
        isStarred: t.isStarred,
        snoozedUntil: t.snoozedUntil,
      });
      for (const m of messagesOf(t.accountId, t.id)) {
        snapshot.messages.push({
          accountId: m.accountId,
          id: m.id,
          labelIds: [...m.labelIds],
          isUnread: m.isUnread,
          isStarred: m.isStarred,
        });
      }
      applyAction(t, action);
    }
    if (action.kind !== 'deleteForever') undoGroups[gestureId] = snapshot;
    afterMutation([...touchedByAccount.keys()], [...touchedByAccount.values()].flat());
    // One gesture, one operation summary per account it touched (P6.3).
    return {
      gestureId,
      operations: [...touchedByAccount].map(([accountId, ids]) =>
        gestureOp(accountId, `Update ${ids.length} conversation${ids.length === 1 ? '' : 's'}`),
      ),
    };
  },
  action_undo: (args) => {
    const gestureId = String(args.gestureId);
    const snap = undoGroups[gestureId];
    if (!snap) throw new Error('undo_expired');
    for (const s of snap.threads) {
      const t = findThread(s.accountId, s.id);
      if (!t) continue;
      t.labelIds = [...s.labelIds];
      t.isStarred = s.isStarred;
      t.snoozedUntil = s.snoozedUntil;
    }
    for (const s of snap.messages) {
      const m = database().messages.find((x) => x.accountId === s.accountId && x.id === s.id);
      if (!m) continue;
      m.labelIds = [...s.labelIds];
      m.isUnread = s.isUnread;
      m.isStarred = s.isStarred;
    }
    for (const s of snap.threads) {
      const t = findThread(s.accountId, s.id);
      if (t) recomputeThread(t);
    }
    delete undoGroups[gestureId];
    afterMutation(
      snap.threads.map((s) => s.accountId),
      snap.threads.map((s) => s.id),
    );
    return { gestureId, operations: [], failures: [] };
  },
  snooze_set: (args) => {
    const targets = (args.targets as { accountId: string; threadId: string }[]) ?? [];
    const gestureId = typeof args.gestureId === 'string' ? args.gestureId : `snooze-${database().nextOpId++}`;
    const wakeAt = Number(args.wakeAt);
    // The pre-snooze membership is saved, not recomputed (P6.5): Undo restores
    // exactly the labels and timer the conversation had a moment ago.
    const snapshot: UndoSnapshot = { threads: [], messages: [] };
    for (const target of targets) {
      const t = findThread(target.accountId, target.threadId);
      if (!t) continue;
      snapshot.threads.push({
        accountId: t.accountId,
        id: t.id,
        labelIds: [...t.labelIds],
        isStarred: t.isStarred,
        snoozedUntil: t.snoozedUntil,
      });
      for (const m of messagesOf(t.accountId, t.id)) {
        snapshot.messages.push({
          accountId: m.accountId,
          id: m.id,
          labelIds: [...m.labelIds],
          isUnread: m.isUnread,
          isStarred: m.isStarred,
        });
      }
      t.snoozedUntil = wakeAt;
      t.labelIds = t.labelIds.filter((l) => l !== 'INBOX');
      for (const m of messagesOf(t.accountId, t.id)) m.labelIds = m.labelIds.filter((l) => l !== 'INBOX');
    }
    undoGroups[gestureId] = snapshot;
    afterMutation(
      targets.map((t) => t.accountId),
      targets.map((t) => t.threadId),
    );
    return { gestureId, operations: [] };
  },
  snooze_clear: (args) => {
    const targets = (args.targets as { accountId: string; threadId: string }[]) ?? [];
    const gestureId =
      typeof args.gestureId === 'string' ? args.gestureId : `unsnooze-${database().nextOpId++}`;
    for (const target of targets) {
      const t = findThread(target.accountId, target.threadId);
      if (!t) continue;
      delete t.snoozedUntil;
      // Clear means Unsnooze: the conversation returns to the Inbox it was
      // snoozed out of.
      if (!t.labelIds.includes('INBOX')) t.labelIds.push('INBOX');
      for (const m of messagesOf(target.accountId, target.threadId)) {
        if (!m.labelIds.includes('INBOX')) m.labelIds.push('INBOX');
      }
      recomputeThread(t);
    }
    afterMutation(
      targets.map((t) => t.accountId),
      targets.map((t) => t.threadId),
    );
    return { gestureId, operations: [] };
  },
  drafts_upsert: (args) => {
    if (failDraftSaves > 0) {
      failDraftSaves -= 1;
      throw new Error('storage_unavailable');
    }
    const incoming = args.draft as Draft;
    const existing = database().drafts.find((d) => d.localId === incoming.localId);
    // The draft id is allocated by the composer; the fixture only mints one for
    // a caller that sends none (explicit discard/legacy paths).
    const localId =
      existing?.localId ??
      incoming.localId ??
      `draft-${database().drafts.length + 1}-${database().nextOpId++}`;
    const revision = Math.max(existing?.revision ?? 0, incoming.revision ?? 0);
    const saved: FixtureDraft = {
      localId,
      accountId: incoming.accountId,
      fromEmail: incoming.fromEmail,
      mode: incoming.mode,
      toJson: incoming.toJson ?? [],
      ccJson: incoming.ccJson ?? [],
      bccJson: incoming.bccJson ?? [],
      subject: incoming.subject ?? '',
      bodyHtml: incoming.bodyHtml ?? '',
      attachmentsJson: incoming.attachmentsJson ?? [],
      threadId: incoming.threadId,
      inReplyToMessageId: incoming.inReplyToMessageId,
      rfcMessageId: incoming.rfcMessageId,
      updatedAt: FIXED_NOW + database().nextOpId,
      revision,
      state: existing?.state === 'queued' ? 'queued' : 'editing',
    };
    if (existing) Object.assign(existing, saved);
    else database().drafts.push(saved);
    persist();
    emit('store:drafts', { account_id: saved.accountId, draft_id: saved.localId });
    return toDraftDto(saved);
  },
  drafts_get: (args) => {
    const localId = String(args.localId);
    const d = database().drafts.find((x) => x.localId === localId);
    if (!d) throw new Error('draft_not_found');
    return toDraftDto(d);
  },
  drafts_list: (args) => {
    const ids = (args.accountIds as string[]) ?? [];
    const limit = Math.min(100, Math.max(1, Number(args.limit ?? 30)));
    const cursor = args.cursor === undefined ? 0 : Number(args.cursor);
    const all = database()
      .drafts.filter((d) => ids.includes(d.accountId))
      .slice()
      .sort((a, b) => b.updatedAt - a.updatedAt || a.localId.localeCompare(b.localId));
    const page = all.slice(cursor, cursor + limit);
    const next = cursor + limit < all.length ? String(cursor + limit) : undefined;
    return { drafts: page.map(toDraftDto), nextCursor: next } satisfies DraftPage;
  },
  drafts_delete: (args) => {
    db.drafts = db.drafts.filter((d) => d.localId !== args.localId);
    persist();
    return null;
  },
  drafts_send: (args) => {
    const localId = String(args.localId);
    const d = database().drafts.find((x) => x.localId === localId);
    if (!d) throw new Error('draft_not_found');
    const existing = database().outbox.find((o) => o.localId === localId && o.state === 'pending');
    if (d.state === 'queued' || existing) {
      // The queued payload is immutable and the key is (draft, revision): a
      // second send of the same revision is refused rather than duplicated.
      throw {
        code: 'draft_queued',
        message: 'This message is already queued',
        retryable: false,
        detail: { opId: existing?.op_id ?? 0 },
      };
    }
    d.state = 'queued';
    const notBefore = Number(args.notBefore ?? FIXED_NOW);
    const recipients = [...(d.toJson ?? []), ...(d.ccJson ?? []), ...(d.bccJson ?? [])]
      .map((a) => a.e)
      .filter(Boolean);
    const op: FixtureOutboxOp = {
      op_id: database().nextOpId++,
      localId,
      draftId: localId,
      kind: 'send',
      state: 'pending',
      notBefore,
      createdAt: FIXED_NOW,
      accountId: d.accountId,
      subject: d.subject,
      recipientSummary: recipients.slice(0, 3).join(', '),
      action: args.archiveAfterSend ? 'Send and archive' : 'Send',
      scheduledAt: notBefore > FIXED_NOW ? notBefore : null,
      scheduledLocalTime: notBefore > FIXED_NOW ? localWallTime(new Date(notBefore)) : null,
      scheduledTimezone: notBefore > FIXED_NOW ? timezoneName() : null,
      retryAt: null,
      startedAt: null,
      completedAt: null,
      errorCode: null,
      errorMessage: null,
      revision: Number(args.revision ?? d.revision),
      requiresDuplicateAck: false,
      payload: `raw-mime-body-for-${localId}`,
    };
    database().outbox.push(op);
    persist();
    emitOutboxState(op.accountId);
    return { opId: op.op_id, notBefore };
  },
  send_cancel: (args) => {
    const opId = Number(args.opId);
    const op = database().outbox.find((o) => o.op_id === opId);
    if (!op || op.state !== 'pending') {
      // Too late: the state travels with the error so the UI can say what
      // actually happened instead of claiming the undo worked.
      throw {
        code: 'send_undo_expired',
        message:
          op?.state === 'done'
            ? 'This message was already sent'
            : op?.state === 'uncertain'
              ? 'Sift handed this message to the provider and cannot prove it did not send'
              : 'This message has already left the queue',
        retryable: false,
        detail: { state: op?.state ?? 'done', opId, notBefore: op?.notBefore ?? FIXED_NOW },
      };
    }
    op.state = 'cancelled';
    op.completedAt = FIXED_NOW;
    const d = database().drafts.find((x) => x.localId === op.localId);
    if (!d) throw new Error('draft_missing');
    d.state = 'editing';
    persist();
    emitOutboxState(op.accountId);
    return toDraftDto(d);
  },
  outbox_list: (args) => {
    const accountIds = (args.accountIds as string[]) ?? [];
    const states = (args.states as string[] | null) ?? null;
    const limit = Math.max(1, Math.min(500, Number(args.limit ?? 50)));
    const all = database()
      .outbox.filter((o) => accountIds.includes(o.accountId))
      .slice()
      .sort((a, b) => b.op_id - a.op_id);
    const counts = countOps(all);
    const matching = states?.length ? all.filter((o) => states.includes(o.state)) : all;
    const cursor = args.cursor === undefined || args.cursor === null ? null : Number(args.cursor);
    const start = cursor === null ? 0 : matching.findIndex((o) => o.op_id === cursor) + 1;
    const page = matching.slice(start, start + limit);
    const last = page[page.length - 1];
    const nextCursor = last && start + limit < matching.length ? last.op_id : null;
    return {
      operations: page.map(outboxSummary),
      nextCursor,
      total: matching.length,
      counts,
    };
  },
  outbox_get: (args) => {
    const op = database().outbox.find((o) => o.op_id === Number(args.opId));
    if (!op) throw { code: 'not_found', message: 'No such operation', retryable: false };
    return outboxSummary(op);
  },
  outbox_retry: (args) => {
    const op = database().outbox.find((o) => o.op_id === Number(args.opId));
    if (!op) throw { code: 'not_found', message: 'No such operation', retryable: false };
    if (op.state === 'uncertain') {
      if (args.acknowledgeDuplicateRisk !== true) {
        throw {
          code: 'acknowledge_duplicate_risk',
          message: 'Retrying an unconfirmed send may deliver the message twice',
          retryable: false,
          detail: { state: 'uncertain', opId: op.op_id },
        };
      }
      op.requiresDuplicateAck = false;
    } else if (op.state !== 'failed') {
      throw {
        code: 'not_retryable',
        message: 'This operation is not retryable',
        retryable: false,
        detail: { state: op.state, opId: op.op_id },
      };
    }
    op.state = 'pending';
    op.retryAt = null;
    op.errorCode = null;
    op.errorMessage = null;
    op.completedAt = null;
    persist();
    emitOutboxState(op.accountId);
    return outboxSummary(op);
  },
  contacts_suggest: (args) => {
    const q = String(args.q ?? '').toLowerCase();
    return database()
      .messages.flatMap((m) => m.from)
      .filter((a, i, all) => all.findIndex((x) => x.e === a.e) === i)
      .filter((a) => a.e.toLowerCase().includes(q))
      .slice(0, Number(args.limit ?? 5))
      .map((a, i) => ({ email: a.e, name: a.n ?? null, lastUsedAt: FIXED_NOW - i, useCount: 3 }));
  },
  search: (args) => {
    const accountIds = (args.accountIds as string[]) ?? [];
    const q = String(args.q ?? '')
      .trim()
      .toLowerCase();
    const rows = threadRows()
      .filter((t) => accountIds.includes(t.accountId))
      .filter((t) => !q || q.split(/\s+/).every((term) => t.searchText.includes(term)))
      .sort((a, b) => compareTuple(tupleOf(a), tupleOf(b)));
    return { rows: rows.slice(0, 100).map(toThreadRow), generation: 1 } satisfies ThreadsPage;
  },
  attachments_open: (args) => {
    const id = String(args.attachmentId);
    const m = database().messages.find((x) => x.attachments.some((a) => a.id === id));
    if (!m) throw new Error('attachment_part_missing');
    attachmentMeta(m, id);
    openCalls.push(id);
    return null;
  },
  attachments_save_as: (args) => {
    const id = String(args.attachmentId);
    const m = database().messages.find((x) => x.attachments.some((a) => a.id === id));
    if (!m) throw new Error('attachment_part_missing');
    const meta = attachmentMeta(m, id);
    const name = attachmentBasename({ ...meta, id });
    saveAsCalls.push({ attachmentId: id, filename: meta.filename ?? name });
    const path = uniqueDestination(name);
    savedPaths.push(path);
    return { path: `/tmp/sift-e2e/${path}` };
  },
  /** Save All (P2.6): one folder, every non-inline part, no clobbering. */
  attachments_save_all: (args) => {
    const accountId = String(args.accountId);
    const m = database().messages.find((x) => x.accountId === accountId && x.id === String(args.messageId));
    if (!m) throw new Error('message_not_found');
    const files = m.attachments.filter((a) => !a.isInline);
    if (files.length < 2) throw new Error('attachment_save_all_not_applicable');
    let saved = 0;
    const failed: AttachmentRefKey[] = [];
    for (const a of files) {
      if (saveAllFailures.has(a.id)) {
        failed.push({ accountId, attachmentId: a.id });
        continue;
      }
      const name = attachmentBasename(a);
      saveAsCalls.push({ attachmentId: a.id, filename: a.filename ?? name });
      savedPaths.push(uniqueDestination(name));
      saved += 1;
    }
    return { saved, failed } satisfies SaveAllResult;
  },
  attachments_cancel: () => null,
  attachments_add_from_paths: (args) => {
    const paths = (args.paths as string[]) ?? [];
    return paths.map((p): AttachmentRef => ({
      name: p.split('/').pop() ?? p,
      mime: 'application/octet-stream',
      size: 1024,
      path: p,
    }));
  },
  attachments_stage_from_message: (args) => {
    const messageId = String(args.messageId);
    const attachmentId = String(args.attachmentId);
    const m = database().messages.find((x) => x.id === messageId);
    const meta = m?.attachments.find((a) => a.id === attachmentId);
    if (!meta) throw new Error('attachment_not_found');
    return {
      name: meta.filename ?? attachmentId,
      mime: meta.mime,
      size: meta.size,
      path: `/fixture/compose-cache/${String(args.draftId)}/${meta.filename ?? attachmentId}`,
    } satisfies AttachmentRef;
  },
  remote_images_load: (args) => {
    const id = String(args.messageId);
    const m = database().messages.find((x) => x.id === id);
    if (!m) throw new Error('message_not_found');
    return {
      messageId: m.id,
      state: 'ready',
      html: m.html,
      text: m.text,
      remoteImageCount: 0,
      trackerCount: 0,
      darkSafe: true,
      remoteImagesAllowed: true,
    } satisfies MessageBody;
  },
  settings_get: () => database().settings,
  settings_set: (args) => {
    const patch = (args.patch ?? {}) as Partial<Settings>;
    db.settings = { ...database().settings, ...patch };
    persist();
    return db.settings;
  },
  storage_usage: () => usageSnapshot(),
  storage_clear_attachment_cache: () => {
    for (const m of database().messages) for (const a of m.attachments) a.downloaded = false;
    persist();
    return usageSnapshot();
  },
  app_set_badge: () => null,
  app_open_url: (args) => {
    openUrlCalls.push(String(args.url));
    return null;
  },
  unsubscribe: () => ({ method: 'one-click', done: true }),
  diagnostics_export: () => ({ path: '/tmp/sift-e2e/diagnostics.json' }),
  perf_mark: () => null,

  // --- P8.1 Send Later ------------------------------------------------------
  send_reschedule: (args) => {
    const op = outboxOp(Number(args.opId));
    if (op.state !== 'pending') {
      throw {
        code: 'send_already_claimed',
        message: 'This message has already left the queue',
        retryable: false,
        detail: { state: op.state, opId: op.op_id },
      };
    }
    op.notBefore = Number(args.notBefore);
    op.scheduledAt = op.notBefore;
    op.scheduledLocalTime = String(args.scheduledLocalTime ?? '');
    op.scheduledTimezone = String(args.scheduledTimezone ?? '');
    persist();
    emitOutboxState(op.accountId);
    return scheduleHandle(op);
  },
  send_now: (args) => {
    const op = outboxOp(Number(args.opId));
    // Clearing the deadline does not bypass the claim: the op still drains in
    // order, it is simply no longer scheduled.
    op.notBefore = FIXED_NOW;
    op.scheduledAt = null;
    op.scheduledLocalTime = null;
    op.scheduledTimezone = null;
    persist();
    emitOutboxState(op.accountId);
    return scheduleHandle(op);
  },

  // --- P8.2 Reminders -------------------------------------------------------
  reminder_set: (args) => {
    const remindAt = Number(args.remindAt);
    for (const target of (args.targets as { accountId: string; threadId: string }[]) ?? []) {
      const existing = database().reminders.find(
        (r) => r.accountId === target.accountId && r.threadId === target.threadId,
      );
      const row: FixtureReminder = {
        accountId: target.accountId,
        threadId: target.threadId,
        remindAt,
        completedAt: null,
        deliveredAt: null,
        state: remindAt <= now() ? 'due' : 'scheduled',
      };
      if (existing) Object.assign(existing, row);
      else database().reminders.push(row);
      afterMutation([target.accountId], [target.threadId]);
    }
    return reminderRows((args.targets as { accountId: string }[])?.map((t) => t.accountId) ?? []);
  },
  reminder_clear: (args) => {
    const targets = (args.targets as { accountId: string; threadId: string }[]) ?? [];
    const removed = database().reminders.filter((r) =>
      targets.some((t) => t.accountId === r.accountId && t.threadId === r.threadId),
    );
    db.reminders = database().reminders.filter(
      (r) => !targets.some((t) => t.accountId === r.accountId && t.threadId === r.threadId),
    );
    persist();
    for (const accountId of new Set(removed.map((r) => r.accountId))) {
      emit('store:threads', { account_id: accountId, thread_ids: removed.map((r) => r.threadId) });
    }
    return removed.map((r) => reminderRow(r));
  },
  reminders_list: (args) => reminderRows((args.accountIds as string[]) ?? [], Boolean(args.includeCompleted)),

  // --- P8.3 Rules -----------------------------------------------------------
  rules_list: (args) => database().rules.filter((r) => r.accountId === args.accountId),
  rules_upsert: (args) => {
    const incoming = args.rule as Omit<FixtureRule, 'id' | 'revision' | 'lastError'> & { id?: string };
    const expected = args.expectedRevision;
    const existing = incoming.id ? database().rules.find((r) => r.id === incoming.id) : undefined;
    if (existing && expected !== undefined && Number(expected) !== existing.revision) {
      throw {
        code: 'rule_revision_conflict',
        message: 'This rule changed elsewhere; reload it before saving',
        retryable: true,
      };
    }
    const saved: FixtureRule = {
      id: existing?.id ?? `rule-${database().nextRuleId++}`,
      accountId: incoming.accountId,
      name: incoming.name,
      enabled: incoming.enabled,
      match: incoming.match,
      conditions: incoming.conditions,
      actions: incoming.actions,
      sortOrder: Number(incoming.sortOrder ?? 0),
      revision: (existing?.revision ?? 0) + 1,
      lastError: null,
    };
    if (existing) Object.assign(existing, saved);
    else database().rules.push(saved);
    persist();
    return saved;
  },
  rules_delete: (args) => {
    db.rules = database().rules.filter((r) => r.id !== args.ruleId);
    persist();
    return null;
  },
  rules_preview: (args) => {
    const rule = args.rule as FixtureRule;
    const matches = database().threads.filter((t) => t.accountId === rule.accountId && ruleMatches(rule, t));
    const limit = Math.max(0, Math.min(10, Number(args.limit ?? 3)));
    return {
      count: matches.length,
      sample: matches.slice(0, limit).map((t) => toPreviewRow(t)),
    };
  },
  rules_apply_existing: (args) => {
    const rule = database().rules.find((r) => r.id === args.ruleId);
    if (!rule) throw new Error('rule_not_found');
    if (Number(args.revision) !== rule.revision) {
      throw {
        code: 'rule_revision_conflict',
        message: 'This rule changed elsewhere; reload it before applying',
        retryable: true,
      };
    }
    let applied = 0;
    let skipped = 0;
    for (const t of database().threads) {
      if (t.accountId !== rule.accountId || !ruleMatches(rule, t)) continue;
      if (applyRuleActions(rule, t)) applied += 1;
      else skipped += 1;
    }
    afterMutation([rule.accountId], []);
    return { applied, skipped };
  },
  rule_block_sender: (args) => {
    const email = String(args.email).toLowerCase();
    const rule: FixtureRule = {
      id: `rule-${database().nextRuleId++}`,
      accountId: String(args.accountId),
      name: `Block ${args.name ?? email}`,
      enabled: true,
      match: 'any',
      conditions: [{ field: 'sender', op: 'is', value: email }],
      actions: [{ kind: 'junk', labelId: null }],
      sortOrder: 0,
      revision: 1,
      lastError: null,
    };
    database().rules.push(rule);
    persist();
    return rule;
  },

  // --- P8.4 Notifications and VIPs -----------------------------------------
  notifications_state: () => database().notifications,
  notifications_enable: (args) => {
    const next = database().notifications;
    next.enabled = Boolean(args.enabled);
    // The permission is requested (and can be refused) on enable, never on
    // every incoming message: a denied OS prompt must not loop.
    if (next.enabled) next.permission = next.permission === 'unsupported' ? 'unsupported' : 'granted';
    persist();
    return next;
  },
  notifications_update: (args) => {
    const next = database().notifications;
    if (args.filter !== undefined) next.filter = args.filter as FixtureNotificationState['filter'];
    if (args.hideSubject !== undefined) next.hideSubject = Boolean(args.hideSubject);
    if (args.sound !== undefined) next.sound = args.sound as FixtureNotificationState['sound'];
    if (args.accountIds !== undefined) next.accountIds = args.accountIds as string[];
    persist();
    return next;
  },
  vip_list: (args) => database().vips[String(args.accountId)] ?? [],
  vip_set: (args) => {
    const accountId = String(args.accountId);
    const email = String(args.email).toLowerCase();
    const list = database().vips[accountId] ?? [];
    database().vips[accountId] = args.vip ? [...new Set([...list, email])] : list.filter((e) => e !== email);
    persist();
    return database().vips[accountId];
  },
  vip_candidates: (args) => {
    const accountId = String(args.accountId);
    const limit = Math.max(1, Math.min(50, Number(args.limit ?? 20)));
    const seen = new Set<string>();
    const out: { email: string; name: string | null; lastUsedAt: number; useCount: number }[] = [];
    for (const m of database().messages) {
      if (m.accountId !== accountId || m.isSentByMe) continue;
      const email = m.from.e.toLowerCase();
      if (seen.has(email)) continue;
      seen.add(email);
      out.push({ email, name: m.from.n ?? null, lastUsedAt: m.internalDate, useCount: 1 });
      if (out.length >= limit) break;
    }
    return out;
  },

  // --- P8.5 Labels and message management ----------------------------------
  label_rename: (args) => {
    const label = labelOf(String(args.accountId), String(args.labelId));
    const name = String(args.name).trim();
    if (!name) throw { code: 'label_name_required', message: 'A label needs a name', retryable: false };
    const clash = database().labels.find(
      (l) =>
        l.account_id === label.account_id && l.id !== label.id && l.name.toLowerCase() === name.toLowerCase(),
    );
    if (clash) {
      throw {
        code: 'label_name_taken',
        message: 'A label with that name already exists in this account',
        retryable: false,
      };
    }
    label.name = name;
    afterMutation([label.account_id], []);
    return label;
  },
  label_delete: (args) => {
    const accountId = String(args.accountId);
    const labelId = String(args.labelId);
    const label = labelOf(accountId, labelId);
    db.labels = database().labels.filter((l) => !(l.account_id === accountId && l.id === labelId));
    // Organization only: the conversations and every other label stay.
    for (const t of database().threads) {
      if (t.accountId === accountId) t.labelIds = t.labelIds.filter((l) => l !== labelId);
    }
    for (const m of database().messages) {
      if (m.accountId === accountId) m.labelIds = m.labelIds.filter((l) => l !== labelId);
    }
    void label;
    afterMutation([accountId], []);
    return null;
  },
  trash_empty_preview: (args) => ({
    count: trashThreads((args.accountIds as string[]) ?? []).length,
  }),
  trash_empty: (args) => {
    const accountIds = (args.accountIds as string[]) ?? [];
    const targets = trashThreads(accountIds);
    for (const t of targets) {
      db.messages = database().messages.filter((m) => !(m.accountId === t.accountId && m.threadId === t.id));
    }
    db.threads = database().threads.filter(
      (t) => !targets.some((x) => x.accountId === t.accountId && x.id === t.id),
    );
    afterMutation(
      accountIds,
      targets.map((t) => t.id),
    );
    return {
      gestureId: `gesture-empty-${database().nextOpId++}`,
      operations: targets.map((t) => gestureOp(t.accountId, 'Delete forever')),
    };
  },

  // --- P9.3 Native utilities ------------------------------------------------
  message_raw_export: (args) => {
    const message = ownedMessage(String(args.accountId), String(args.messageId));
    const path = `/tmp/sift-e2e/${message.id}.eml`;
    savedPaths.push(path);
    return { path };
  },

  // --- P10.4 Storage cap ----------------------------------------------------
  storage_set_attachment_cap: (args) => {
    attachmentCapBytes = Math.max(0, Number(args.bytes ?? 0));
    return usageSnapshot();
  },
};

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

export async function invokeFixture(cmd: string, args?: Args): Promise<unknown> {
  let summary: Args | undefined;
  try {
    summary = args ? (JSON.parse(JSON.stringify(args)) as Args) : undefined;
  } catch {
    summary = undefined;
  }
  callLog.push({ cmd, at: Date.now(), args: summary });
  const delay = delays[cmd];
  if (delay) await sleep(delay(args ?? {}));
  const handler = handlers[cmd];
  if (!handler) {
    const message = `unimplemented fixture command: ${cmd}`;
    unimplemented.push(message);
    throw new Error(message);
  }
  return handler(args ?? {});
}

export interface FixtureControl {
  ids: { accountA: string; accountB: string };
  control: {
    delay: (cmd: string, ms: number) => void;
    delayFor: (cmd: string, fn: (args: Args) => number) => void;
    clearDelays: () => void;
    calls: () => { cmd: string; at: number; args?: Args }[];
    failAppPasswordFlow: (fail: boolean) => void;
    undoGroups: () => string[];
    accounts: () => Account[];
    callCount: (cmd: string) => number;
    drafts: () => FixtureDraft[];
    /** Reject the next N draft saves, to exercise the storage-failure path. */
    failDraftSaves: (times: number) => void;
    /**
     * Reject the next `times` body fetches of one message, so the reader's
     * provider-error path is reachable (P9.2). Defaults to the first account,
     * which is the account the seed's long thread belongs to.
     */
    failBody: (messageId: string, times: number, accountId?: string) => void;
    /** Body fetches the app made over IPC, in order — cache hits never appear. */
    bodyCalls: () => { accountId: string; messageId: string; at: number }[];
    outbox: () => {
      op_id: number;
      state: string;
      subject: string;
      recipientSummary: string;
      errorMessage: string | null;
      scheduledAt: number | null;
      payload?: string;
    }[];
    /** Force an operation into the state the drain loop would have produced. */
    setOpState: (opId: number, state: FixtureOutboxOp['state']) => unknown;
    /** Seed a large queue without going through the composer. */
    seedOutbox: (count: number, accountId?: string) => void;
    threads: (accountId?: string) => ThreadRow[];
    savedAs: () => { attachmentId: string; filename: string }[];
    /** Destination paths the fixture wrote, in order — collisions are visible. */
    savedPaths: () => string[];
    /** Make Save All fail for one attachment, to exercise the retry path. */
    failSaveAll: (attachmentId: string | null) => void;
    /** One account's provider operation starts failing (P4.6). */
    failAccount: (accountId: string, failure: 'connectivity' | 'auth', message: string) => void;
    connectivity: () => ConnectivityState[];
    opened: () => string[];
    openedUrls: () => string[];
    unimplemented: () => string[];
    mutateThread: (accountId: string, threadId: string, patch: Partial<FixtureThread>) => void;
    injectNewMail: (accountId: string, id: string, subject: string) => void;
    reset: () => void;
    /** Move the fixture clock and run due sends/reminders (P8.1/P8.2). */
    advanceClock: (ms: number) => void;
    clock: () => number;
    reminders: () => ReminderDto[];
    rules: () => FixtureRule[];
    notifications: () => FixtureNotificationState;
    setNotifications: (patch: Partial<FixtureNotificationState>) => void;
    notificationsSent: () => {
      accountId: string;
      threadId: string;
      subject: string | null;
      hidden: boolean;
    }[];
    clickNotification: () => { accountId: string; threadId: string };
    notifyBurst: (count: number, threadId: string) => { count: number; threadId: string };
  };
}

export function installControl(): void {
  const w = window as unknown as { __siftFixture?: FixtureControl };
  if (w.__siftFixture) return;
  w.__siftFixture = {
    ids: { accountA: ACCOUNT_A, accountB: ACCOUNT_B },
    control: {
      delay: (cmd, ms) => {
        delays[cmd] = () => ms;
      },
      delayFor: (cmd, fn) => {
        delays[cmd] = fn;
      },
      clearDelays: () => {
        for (const k of Object.keys(delays)) delete delays[k];
      },
      calls: () => callLog.map((c) => ({ ...c })),
      failAppPasswordFlow: (fail) => {
        failAppPassword = fail;
      },
      undoGroups: () => Object.keys(undoGroups),
      accounts: () => database().accounts.map((a) => ({ ...a })),
      callCount: (cmd) => callLog.filter((c) => c.cmd === cmd).length,
      drafts: () => database().drafts.map((d) => ({ ...d })),
      failDraftSaves: (times) => {
        failDraftSaves = Math.max(0, Math.floor(times));
      },
      failBody: (messageId, times, accountId) => {
        const account = accountId ?? database().accounts[0]?.id ?? ACCOUNT_A;
        bodyFailures[`${account}:${messageId}`] = Math.max(0, Math.floor(times));
      },
      bodyCalls: () => bodyFetches.map((c) => ({ ...c })),
      outbox: () =>
        database().outbox.map((o) => ({
          op_id: o.op_id,
          state: o.state,
          subject: o.subject,
          recipientSummary: o.recipientSummary,
          errorMessage: o.errorMessage,
          scheduledAt: o.scheduledAt,
          payload: o.payload,
        })),
      /** Drive one operation to a state the drain loop would have produced. */
      setOpState: (opId, state) => {
        const op = database().outbox.find((o) => o.op_id === opId);
        if (!op) throw new Error('op_not_found');
        op.state = state;
        if (state === 'failed') {
          op.errorCode = 'smtp_rejected';
          op.errorMessage = 'The provider rejected this message';
          op.retryAt = FIXED_NOW + 60_000;
        }
        if (state === 'uncertain') {
          op.requiresDuplicateAck = true;
          op.errorMessage = 'Sift could not confirm whether the provider accepted this message';
        }
        if (state === 'inflight') op.startedAt = FIXED_NOW;
        if (state === 'done' || state === 'cancelled') op.completedAt = FIXED_NOW;
        persist();
        emitOutboxState(op.accountId);
        return outboxSummary(op);
      },
      /**
       * Seed a large queue to prove the panel stays small (P6.6). Deliberately
       * not persisted: 10,000 rows would overflow the fixture's localStorage,
       * and the property under test is the panel's row count, not the store.
       */
      seedOutbox: (count, accountId) => {
        const account = accountId ?? database().accounts[0]?.id ?? ACCOUNT_A;
        for (let i = 0; i < count; i++) {
          database().outbox.push({
            op_id: database().nextOpId++,
            localId: `seeded-${i}`,
            draftId: `seeded-${i}`,
            kind: 'label',
            state: 'pending',
            notBefore: FIXED_NOW + i,
            createdAt: FIXED_NOW,
            accountId: account,
            subject: `Seeded operation ${i}`,
            recipientSummary: '',
            action: 'Archive',
            scheduledAt: null,
            retryAt: null,
            startedAt: null,
            completedAt: null,
            errorCode: null,
            errorMessage: null,
            revision: 1,
            requiresDuplicateAck: false,
            payload: `raw-mime-body-for-seeded-${i}`,
          });
        }
        emitOutboxState(account);
      },
      threads: (accountId) =>
        threadRows()
          .filter((t) => !accountId || t.accountId === accountId)
          .map((t) => ({ ...t })),
      savedAs: () => [...saveAsCalls],
      savedPaths: () => [...savedPaths],
      failSaveAll: (attachmentId) => {
        saveAllFailures.clear();
        if (attachmentId) saveAllFailures.add(attachmentId);
      },
      failAccount: (accountId, failure, message) => failAccount(accountId, failure, message),
      connectivity: () => database().accounts.map((a) => connectivityRow(a.id)),
      opened: () => [...openCalls],
      openedUrls: () => [...openUrlCalls],
      unimplemented: () => [...unimplemented],
      mutateThread: (accountId, threadId, patch) => {
        const t = findThread(accountId, threadId);
        if (!t) throw new Error('thread_not_found');
        Object.assign(t, patch);
        afterMutation([accountId], [threadId]);
      },
      injectNewMail: (accountId, id, subject) => {
        const ts = FIXED_NOW + 60_000;
        const m: FixtureMessage = {
          id: `${id}-m1`,
          threadId: id,
          accountId,
          internalDate: ts,
          from: { e: 'newest@example.test', n: 'New Sender' },
          to: [{ e: accountId === ACCOUNT_A ? 'ada@example.test' : 'ben@example.test' }],
          cc: [],
          bcc: [],
          subject,
          snippet: 'Brand new mail',
          isUnread: true,
          isStarred: false,
          isDraft: false,
          isSentByMe: false,
          labelIds: ['INBOX', 'UNREAD'],
          hasAttachments: false,
          attachments: [],
          bodyState: 'fetched',
          html: `<p>${subject}</p>`,
          text: subject,
        };
        database().messages.push(m);
        const t: FixtureThread = {
          accountId,
          id,
          subject,
          snippet: 'Brand new mail',
          participants: [{ e: 'newest@example.test', n: 'New Sender' }],
          lastMessageAt: ts,
          messageCount: 1,
          unreadCount: 1,
          isStarred: false,
          hasAttachments: false,
          labelIds: ['INBOX', 'UNREAD'],
          searchText: `${subject} newest@example.test new sender`.toLowerCase(),
        };
        database().threads.push(t);
        afterMutation([accountId], [id]);
        // The runtime notifies after the metadata commit, exactly once per
        // message unless a burst was grouped (P8.4).
        notifyThread(accountId, t);
        emitBadge();
      },
      /**
       * Move the fixture clock and run everything the runtime would run at that
       * moment (P8.1/P8.2): due sends are claimed, due reminders are delivered
       * or marked due. This is what makes "sleep across the due time" a real
       * transition instead of a hand-waved assertion.
       */
      advanceClock: (ms) => {
        setClock(clockSkew + ms);
        const at = now();
        for (const op of database().outbox) {
          if (op.state !== 'pending' || op.notBefore > at) continue;
          op.state = 'done';
          op.completedAt = at;
          const d = database().drafts.find((x) => x.localId === op.localId);
          if (d) d.state = 'sent';
          emitOutboxState(op.accountId);
        }
        for (const r of database().reminders) {
          if (r.completedAt != null || r.remindAt > at) continue;
          // Delivery is recorded before the notification is attempted (P8.2):
          // a denied OS prompt leaves the reminder `due`, visible, not lost.
          const cfg = database().notifications;
          if (cfg.enabled && cfg.permission === 'granted' && cfg.filter !== 'off') {
            r.deliveredAt = at;
            r.state = 'delivered';
          } else {
            r.state = 'due';
          }
          emit('store:threads', { account_id: r.accountId, thread_ids: [r.threadId] });
        }
        persist();
        emitBadge();
      },
      /** The clock the fixture currently reports, for assertions. */
      clock: () => now(),
      reminders: () =>
        reminderRows(
          database().accounts.map((a) => a.id),
          true,
        ),
      rules: () => database().rules.map((r) => ({ ...r })),
      notifications: () => ({ ...database().notifications }),
      /** Patch the delivery policy the way the Settings panel does (P8.4). */
      setNotifications: (patch) => Object.assign(database().notifications, patch),
      /** Every `notify:new` the runtime emitted, in order (P8.4). */
      notificationsSent: () => notifyLog.map((n) => ({ ...n })),
      /**
       * The user clicks the notification banner: the runtime focuses the window
       * and tells the frontend which conversation to open.
       */
      clickNotification: () => {
        if (!lastNotified) throw new Error('no_notification');
        emit('nav:open-thread', { ...lastNotified });
        return { ...lastNotified };
      },
      /** One grouped summary for a burst, the way the runtime coalesces it. */
      notifyBurst: (count, threadId) => {
        const accountId = database().accounts[0]?.id ?? ACCOUNT_A;
        const t = findThread(accountId, threadId) ?? threadRows()[0];
        if (!t) throw new Error('no_thread');
        lastNotified = { accountId, threadId: t.id };
        notifyLog.push({ accountId, threadId: t.id, subject: t.subject, hidden: false });
        emit('notify:new', {
          accountId,
          threadId: t.id,
          title: `${count} new messages`,
          subject: null,
          body: `${count} new messages in ${t.subject}`,
          hidden: false,
        });
        return { count, threadId: t.id };
      },
      reset: () => {
        for (const k of Object.keys(delays)) delete delays[k];
        for (const k of Object.keys(bodyFailures)) delete bodyFailures[k];
        bodyFetches.length = 0;
        callLog.length = 0;
        unimplemented.length = 0;
        saveAsCalls.length = 0;
        savedPaths.length = 0;
        saveAllFailures.clear();
        openCalls.length = 0;
        openUrlCalls.length = 0;
        undoGroups = {};
        network = true;
        health = {};
        setClock(0);
        notifyLog.length = 0;
        lastNotified = null;
        attachmentCapBytes = 2 * 1024 * 1024 * 1024;
        try {
          window.localStorage.removeItem(FIXTURE_DB_KEY);
        } catch {
          /* ignore */
        }
        db = seedDatabase(scenarioFromLocation());
        persist();
      },
    },
  };
}

/** Test-only seam: replace the database (used by the Vitest contract check). */
export function setDatabase(next: FixtureDb): void {
  db = next;
}

export const FIXTURE_ACCOUNTS = { A: ACCOUNT_A, B: ACCOUNT_B, NOW: FIXED_NOW };
