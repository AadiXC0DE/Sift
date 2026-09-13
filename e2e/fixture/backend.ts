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
  Draft,
  Label,
  MessageBody,
  MessageMeta,
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
  type FixtureThread,
  seedDatabase,
  type Scenario,
} from './dataset';

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
const openCalls: string[] = [];
const openUrlCalls: string[] = [];
const delays: Record<string, (args: Args) => number> = {};
let failAppPassword = false;

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
    case 'spam':
      return has('SPAM');
    case 'trash':
      return has('TRASH');
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

/** IPC boundary for drafts: store-only revision/state stay in the fixture. */
function toDraftDto(d: FixtureDraft): Draft {
  return {
    localId: d.localId,
    accountId: d.accountId,
    mode: d.mode,
    toJson: d.toJson,
    ccJson: d.ccJson,
    bccJson: d.bccJson,
    subject: d.subject,
    bodyHtml: d.bodyHtml,
    attachmentsJson: d.attachmentsJson,
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
    attachmentCacheLimitBytes: 2 * 1024 * 1024 * 1024,
    totalBytes: metadata + bodies + attachments + draftCache,
    computedAt: FIXED_NOW,
  };
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
    persist();
    emit('store:threads', { account_id: id, thread_ids: [] });
    emit('store:labels', { account_id: id });
    return null;
  },
  system_info: () => ({ version: 'e2e-fixture', oauth_available: false, demo: false }),
  sync_now: () => null,
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
  message_raw_source: (args) => {
    const id = String(args.messageId);
    const m = database().messages.find((x) => x.id === id);
    if (!m) throw new Error('message_not_found');
    return `From: ${m.from.e}\r\nSubject: ${m.subject}\r\n\r\n${m.text}\r\n`;
  },
  threads_action: (args) => {
    const req = args.req as ThreadAction;
    const group = `undo-${database().nextOpId++}`;
    const snapshot: UndoSnapshot = { threads: [], messages: [] };
    const touched: string[] = [];
    for (const id of req.threadIds) {
      const t = findThread(req.accountId, id);
      if (!t) continue;
      touched.push(id);
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
      applyAction(t, req.action);
    }
    if (req.action.kind !== 'deleteForever') undoGroups[group] = snapshot;
    afterMutation([req.accountId], touched);
    return { undo_group: group };
  },
  action_undo: (args) => {
    const group = String(args.undoGroup);
    const snap = undoGroups[group];
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
    delete undoGroups[group];
    afterMutation(
      snap.threads.map((s) => s.accountId),
      snap.threads.map((s) => s.id),
    );
    return null;
  },
  snooze_set: (args) => {
    const accountId = String(args.accountId);
    const ids = (args.threadIds as string[]) ?? [];
    const wakeAt = Number(args.wakeAt);
    for (const id of ids) {
      const t = findThread(accountId, id);
      if (!t) continue;
      t.snoozedUntil = wakeAt;
      t.labelIds = t.labelIds.filter((l) => l !== 'INBOX');
      for (const m of messagesOf(accountId, id)) m.labelIds = m.labelIds.filter((l) => l !== 'INBOX');
    }
    afterMutation([accountId], ids);
    return { undo_group: `snooze-${database().nextOpId++}` };
  },
  snooze_clear: (args) => {
    const accountId = String(args.accountId);
    const ids = (args.threadIds as string[]) ?? [];
    for (const id of ids) {
      const t = findThread(accountId, id);
      if (!t) continue;
      delete t.snoozedUntil;
      if (!t.labelIds.includes('INBOX')) t.labelIds.push('INBOX');
    }
    afterMutation([accountId], ids);
    return null;
  },
  drafts_upsert: (args) => {
    const incoming = args.draft as Draft;
    const existing = incoming.localId
      ? database().drafts.find((d) => d.localId === incoming.localId)
      : undefined;
    const saved: FixtureDraft = {
      localId:
        existing?.localId ??
        incoming.localId ??
        `draft-${database().drafts.length + 1}-${database().nextOpId++}`,
      accountId: incoming.accountId,
      mode: incoming.mode,
      toJson: incoming.toJson ?? [],
      ccJson: incoming.ccJson ?? [],
      bccJson: incoming.bccJson ?? [],
      subject: incoming.subject ?? '',
      bodyHtml: incoming.bodyHtml ?? '',
      attachmentsJson: incoming.attachmentsJson ?? [],
      updatedAt: FIXED_NOW + database().nextOpId,
      revision: (existing?.revision ?? 0) + 1,
      state: 'editing',
    };
    if (existing) Object.assign(existing, saved);
    else database().drafts.push(saved);
    persist();
    emit('store:drafts', { account_id: saved.accountId, draft_id: saved.localId });
    return toDraftDto(saved);
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
    d.state = 'queued';
    const op = {
      op_id: database().nextOpId++,
      localId,
      state: 'pending' as const,
      notBefore: FIXED_NOW + Number(args.undoDelayMs ?? 0),
      createdAt: FIXED_NOW,
      accountId: d.accountId,
      subject: d.subject,
    };
    database().outbox.push(op);
    persist();
    emit('outbox:state', { op_id: op.op_id, state: op.state });
    return { op_id: op.op_id };
  },
  send_cancel: (args) => {
    const opId = Number(args.opId);
    const op = database().outbox.find((o) => o.op_id === opId);
    if (!op) throw new Error('send_undo_expired');
    if (op.state !== 'pending') throw new Error('send_undo_expired');
    op.state = 'cancelled';
    const d = database().drafts.find((x) => x.localId === op.localId);
    if (!d) throw new Error('draft_missing');
    d.state = 'editing';
    persist();
    emit('outbox:state', { op_id: op.op_id, state: op.state });
    return toDraftDto(d);
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
    saveAsCalls.push({ attachmentId: id, filename: meta.filename ?? 'attachment' });
    return { path: `/tmp/sift-e2e/${meta.filename ?? 'attachment'}` };
  },
  attachments_add_from_paths: (args) => {
    const paths = (args.paths as string[]) ?? [];
    return paths.map((p): AttachmentRef => ({
      name: p.split('/').pop() ?? p,
      mime: 'application/octet-stream',
      size: 1024,
      path: p,
    }));
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
    outbox: () => { op_id: number; state: string; subject: string }[];
    threads: (accountId?: string) => ThreadRow[];
    savedAs: () => { attachmentId: string; filename: string }[];
    opened: () => string[];
    openedUrls: () => string[];
    unimplemented: () => string[];
    mutateThread: (accountId: string, threadId: string, patch: Partial<FixtureThread>) => void;
    injectNewMail: (accountId: string, id: string, subject: string) => void;
    reset: () => void;
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
      outbox: () => database().outbox.map((o) => ({ op_id: o.op_id, state: o.state, subject: o.subject })),
      threads: (accountId) =>
        threadRows()
          .filter((t) => !accountId || t.accountId === accountId)
          .map((t) => ({ ...t })),
      savedAs: () => [...saveAsCalls],
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
      },
      reset: () => {
        for (const k of Object.keys(delays)) delete delays[k];
        callLog.length = 0;
        unimplemented.length = 0;
        saveAsCalls.length = 0;
        openCalls.length = 0;
        openUrlCalls.length = 0;
        undoGroups = {};
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
