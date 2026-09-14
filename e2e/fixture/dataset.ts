/**
 * Deterministic seed data for the browser E2E fixture backend.
 *
 * Every value is derived from fixed constants: no `Date.now()`, no random ids,
 * no network. The same seed is produced on every page load so list order,
 * pagination boundaries, label counts and screenshot geometry are reproducible.
 *
 * Field names are the exact serialized shapes declared in
 * `src/app/ipc/types.ts`; `src/test/fixture-contract.test.ts` parses that file
 * and fails when this module drifts from it.
 */
import type {
  Account,
  Address,
  AttachmentMeta,
  Label,
  MessageMeta,
  Settings,
  SyncStatus,
  ThreadRow,
} from '../../src/app/ipc/types';
import { defaultSettings } from '../../src/app/ipc/types';

/** Fixed clock: 2026-01-15T12:00:00Z. */
export const FIXED_NOW = 1768478400000;

export const ACCOUNT_A = 'acc-a';
export const ACCOUNT_B = 'acc-b';

/**
 * The reader's long conversation (P9.2): 200 messages, enough to page the
 * reader's body fetches and prove newest-first ordering. The four highest
 * message numbers carry the rendering cases the reader expands first — RTL,
 * CJK, a message with no HTML part, and a message with no body at all.
 */
const LONG_THREAD_ID = 'long-0';
const LONG_THREAD_MESSAGES = 200;

export type Scenario = 'default' | 'empty';

/** A message row plus the rendered body the fixture serves for it. */
export interface FixtureMessage extends MessageMeta {
  threadId: string;
  accountId: string;
  html: string;
  text: string;
}

/** A thread row plus the fixture-only bookkeeping the mock needs. */
export interface FixtureThread extends ThreadRow {
  /** Snippet hash of the newest message; kept for stable search results. */
  searchText: string;
}

export interface FixtureAttachment extends AttachmentMeta {
  /** Deterministic stand-in for cached bytes; never exposed over IPC. */
  bytes: string;
}

export interface FixtureDraft {
  localId: string;
  accountId: string;
  fromEmail?: string;
  mode: string;
  toJson: Address[];
  ccJson: Address[];
  bccJson: Address[];
  subject: string;
  bodyHtml: string;
  attachmentsJson: { name: string; mime: string; size: number; path: string }[];
  threadId?: string;
  inReplyToMessageId?: string;
  rfcMessageId?: string;
  updatedAt: number;
  revision: number;
  state: 'editing' | 'queued' | 'sent' | 'failed';
}

export interface FixtureOutboxOp {
  op_id: number;
  localId: string;
  kind: string;
  state: 'pending' | 'inflight' | 'uncertain' | 'done' | 'failed' | 'cancelled';
  notBefore: number;
  createdAt: number;
  accountId: string;
  subject: string;
  /** Lightweight summary composed at enqueue; the payload never lands here. */
  recipientSummary: string;
  action: string;
  scheduledAt: number | null;
  /** Exact intended local wall time `YYYY-MM-DDTHH:MM` for a scheduled send (P8.1). */
  scheduledLocalTime?: string | null;
  /** The IANA zone that wall time belongs to. */
  scheduledTimezone?: string | null;
  retryAt: number | null;
  startedAt: number | null;
  completedAt: number | null;
  errorCode: string | null;
  errorMessage: string | null;
  revision: number;
  draftId: string;
  requiresDuplicateAck: boolean;
  /** Internal to the fixture: the raw body never crosses the IPC boundary. */
  payload?: string;
}

export interface FixtureDb {
  version: 1;
  scenario: Scenario;
  accounts: Account[];
  labels: Label[];
  threads: FixtureThread[];
  messages: FixtureMessage[];
  drafts: FixtureDraft[];
  outbox: FixtureOutboxOp[];
  settings: Settings;
  sync: SyncStatus[];
  nextOpId: number;
  /** Local message reminders (P8.2): no provider label, no timestamp change. */
  reminders: FixtureReminder[];
  rules: FixtureRule[];
  /** VIP addresses per account (P8.4). */
  vips: Record<string, string[]>;
  notifications: FixtureNotificationState;
  nextRuleId: number;
}

export interface FixtureReminder {
  accountId: string;
  threadId: string;
  remindAt: number;
  completedAt: number | null;
  deliveredAt: number | null;
  /** `scheduled` | `due` (deadline passed, delivery unconfirmed) | `delivered`. */
  state: 'scheduled' | 'due' | 'delivered';
}

export interface FixtureRule {
  id: string;
  accountId: string;
  name: string;
  enabled: boolean;
  match: 'all' | 'any';
  conditions: { field: string; op: string; value: string }[];
  actions: { kind: string; labelId: string | null }[];
  sortOrder: number;
  revision: number;
  lastError: string | null;
}

export interface FixtureNotificationState {
  enabled: boolean;
  permission: 'granted' | 'denied' | 'prompt' | 'unsupported';
  filter: 'off' | 'inbox' | 'vip';
  hideSubject: boolean;
  sound: 'none' | 'native';
  accountIds: string[];
}

const addr = (e: string, n?: string): Address => (n ? { e, n } : { e });

/** Seed participants, shared by the specs and the reader's long thread (P9.2). */
const ADA = addr('ada@example.test', 'Ada Lovelace');
const BEN = addr('ben@example.test', 'Ben Ortiz');

function account(
  id: string,
  email: string,
  display_name: string,
  color: string,
  sort_order: number,
): Account {
  return {
    id,
    provider: 'gmail',
    email,
    display_name,
    avatar_url: null,
    color,
    auth_kind: 'app_password',
    history_id: null,
    sync_state: 'partial',
    last_sync_at: FIXED_NOW - 60_000,
    created_at: FIXED_NOW - 30 * 86_400_000,
    sort_order,
    signature_html: `<p>— ${display_name}</p>`,
  };
}

function label(
  account_id: string,
  id: string,
  name: string,
  kind: string,
  unread_count: number,
  total_count: number,
  sort_order: number,
): Label {
  return {
    account_id,
    id,
    name,
    kind,
    color_bg: null,
    color_fg: null,
    visible: true,
    unread_count,
    total_count,
    sort_order,
    depth: 0,
  };
}

interface ThreadSpec {
  accountId: string;
  id: string;
  subject: string;
  snippet: string;
  lastMessageAt: number;
  labelIds: string[];
  unread: boolean;
  starred?: boolean;
  from: string;
  fromName: string;
  blocksHtml?: string;
  /**
   * Further messages in the same conversation. Membership is decided per
   * message (P3.6), and a Sent-only row names the recipients, which is only
   * observable on a conversation with more than one participant.
   */
  extra?: { labelIds: string[]; from: Address; unread: boolean }[];
}

function threadSpecs(): ThreadSpec[] {
  const specs: ThreadSpec[] = [];
  const push = (s: ThreadSpec) => specs.push(s);

  // Group 1 — 30 adjacent rows from the blue account. These are the newest
  // inbox rows in unified scope, so they are adjacent by construction.
  for (let i = 0; i < 30; i++) {
    push({
      accountId: ACCOUNT_A,
      id: `blue-${String(i).padStart(2, '0')}`,
      subject: `Blue run ${String(i).padStart(2, '0')}`,
      snippet: `Blue account snippet ${i}`,
      lastMessageAt: FIXED_NOW - i * 60_000,
      labelIds: ['INBOX'],
      unread: i % 3 === 0,
      starred: i === 2 || i === 3,
      from: ADA.e,
      fromName: ADA.n!,
      blocksHtml: i === 0 ? 'Blue run 00 body with the attachment set.' : undefined,
    });
  }

  // Group 2 — interleaved accounts, so unified rows alternate stripes.
  for (let i = 0; i < 40; i++) {
    const a = i % 2 === 0 ? ACCOUNT_B : ACCOUNT_A;
    const from = a === ACCOUNT_A ? ADA : BEN;
    push({
      accountId: a,
      id: `mixed-${String(i).padStart(2, '0')}`,
      subject: i === 33 ? 'Invoice 2026-01 for Client Work' : `Mixed ${String(i).padStart(2, '0')}`,
      snippet: `Interleaved snippet ${i}`,
      lastMessageAt: FIXED_NOW - (30 + i) * 60_000,
      labelIds: ['INBOX', ...(i === 33 ? ['Label_A_Client'] : [])],
      unread: i % 4 === 0,
      from: from.e,
      fromName: from.n!,
    });
  }

  // Group 3 — filler that brings the inbox to 400 rows before the reader's
  // long conversation (P9.2) adds one more.
  for (let i = 0; i < 79; i++) {
    const a = i % 2 === 0 ? ACCOUNT_B : ACCOUNT_A;
    const from = a === ACCOUNT_A ? ADA : BEN;
    push({
      accountId: a,
      id: `fill-${String(i).padStart(2, '0')}`,
      subject: i < 4 ? `Invoice 2026-0${i + 1} receipt` : `Fill ${String(i).padStart(2, '0')}`,
      snippet: `Filler snippet ${i}`,
      lastMessageAt: FIXED_NOW - (70 + i) * 60_000,
      labelIds: ['INBOX'],
      unread: false,
      from: from.e,
      fromName: from.n!,
    });
  }

  // Group 4 — 251 threads sharing one timestamp across two accounts. Pageable
  // only when the cursor carries the full (timestamp, account, id) tuple.
  for (let i = 0; i < 251; i++) {
    const a = i < 126 ? ACCOUNT_A : ACCOUNT_B;
    const from = a === ACCOUNT_A ? ADA : BEN;
    const n = String(a === ACCOUNT_A ? i : i - 126).padStart(3, '0');
    push({
      accountId: a,
      id: `needle-${a === ACCOUNT_A ? 'a' : 'b'}-${n}`,
      subject: `needle report ${a === ACCOUNT_A ? 'A' : 'B'}${n}`,
      snippet: `Equal timestamp ${n}`,
      lastMessageAt: FIXED_NOW - 20_000_000,
      labelIds: ['INBOX'],
      unread: false,
      from: from.e,
      fromName: from.n!,
    });
  }

  // Archive — no Inbox, not Trash/Junk/Draft/Sent.
  for (let i = 0; i < 5; i++) {
    push({
      accountId: i % 2 ? ACCOUNT_B : ACCOUNT_A,
      id: `archived-${i}`,
      subject: `Archived ${i}`,
      snippet: `Archived snippet ${i}`,
      lastMessageAt: FIXED_NOW - (200 + i) * 60_000,
      labelIds: ['ARCHIVE', ...(i === 0 ? ['Label_A_Client'] : [])],
      unread: false,
      from: i % 2 ? BEN.e : ADA.e,
      fromName: i % 2 ? BEN.n! : ADA.n!,
    });
  }

  // Spam / Trash / Sent / Drafts.
  for (let i = 0; i < 2; i++) {
    push({
      accountId: ACCOUNT_A,
      id: `spam-${i}`,
      subject: `Spam ${i}`,
      snippet: `Spam snippet ${i}`,
      lastMessageAt: FIXED_NOW - (300 + i) * 60_000,
      labelIds: ['SPAM'],
      unread: true,
      from: 'offers@spam.example',
      fromName: 'Offer Bot',
    });
  }
  for (let i = 0; i < 4; i++) {
    push({
      accountId: i % 2 ? ACCOUNT_B : ACCOUNT_A,
      id: `trashed-${i}`,
      subject: `Trashed ${i}`,
      snippet: `Trashed snippet ${i}`,
      lastMessageAt: FIXED_NOW - (400 + i) * 60_000,
      labelIds: ['TRASH'],
      unread: false,
      from: i % 2 ? BEN.e : ADA.e,
      fromName: i % 2 ? BEN.n! : ADA.n!,
    });
  }
  for (let i = 0; i < 3; i++) {
    push({
      accountId: ACCOUNT_A,
      id: `sent-${i}`,
      subject: i === 0 ? 'Sent with attachment' : `Sent ${i}`,
      snippet: `Sent snippet ${i}`,
      lastMessageAt: FIXED_NOW - (500 + i) * 60_000,
      labelIds: ['SENT'],
      unread: false,
      from: ADA.e,
      fromName: ADA.n!,
    });
  }
  push({
    accountId: ACCOUNT_A,
    id: 'draft-0',
    subject: 'Draft: quarterly plan',
    snippet: 'Draft snippet 0',
    lastMessageAt: FIXED_NOW - 600 * 60_000,
    labelIds: ['DRAFT'],
    unread: false,
    from: ADA.e,
    fromName: ADA.n!,
  });

  // Snoozed: still inbox members, hidden until their wake time.
  push({
    accountId: ACCOUNT_A,
    id: 'snoozed-0',
    subject: 'Snoozed until tomorrow',
    snippet: 'Snoozed snippet 0',
    lastMessageAt: FIXED_NOW - 700 * 60_000,
    labelIds: ['INBOX'],
    unread: true,
    from: ADA.e,
    fromName: ADA.n!,
  });

  // A Sent conversation with a reply the reader archived. The conversation
  // holds the sender *and* the recipient, so a Sent-only row must name the
  // recipient rather than echoing the account that sent it (P3.6).
  push({
    accountId: ACCOUNT_A,
    id: 'sent-thread-0',
    subject: 'Sent to the client',
    snippet: 'Sent snippet with a reply',
    lastMessageAt: FIXED_NOW - 750 * 60_000,
    labelIds: ['SENT'],
    unread: false,
    from: ADA.e,
    fromName: ADA.n!,
    extra: [
      {
        labelIds: ['ARCHIVE'],
        from: addr('client@example.test', 'Client Team'),
        unread: false,
      },
    ],
  });

  // One inbox message beside one trashed message. All Mail membership is per
  // message, so this conversation stays in All Mail; Trash asks whether every
  // message is trashed, so Trash excludes it (P3.6).
  push({
    accountId: ACCOUNT_A,
    id: 'mixed-thread-0',
    subject: 'Half-trashed conversation',
    snippet: 'One inbox message, one trashed',
    lastMessageAt: FIXED_NOW - 800 * 60_000,
    labelIds: ['INBOX'],
    unread: false,
    from: ADA.e,
    fromName: ADA.n!,
    extra: [{ labelIds: ['TRASH'], from: ADA, unread: false }],
  });

  // The reader's long conversation (P9.2). Deliberately old: `blue-00` stays
  // the newest Inbox row the reading spec asserts on first paint, and the
  // 200-message body is generated in `seedDatabase`.
  push({
    accountId: ACCOUNT_A,
    id: LONG_THREAD_ID,
    subject: 'Long thread 00',
    snippet: 'Two hundred messages',
    lastMessageAt: FIXED_NOW - 900 * 60_000,
    labelIds: ['INBOX'],
    unread: false,
    from: ADA.e,
    fromName: ADA.n!,
  });
  return specs;
}

function attachmentSet(accountId: string, messageId: string): FixtureAttachment[] {
  const base = `${accountId}:${messageId}`;
  return [
    {
      id: `${base}:att-pdf`,
      filename: 'invoice.pdf',
      mime: 'application/pdf',
      size: 24_576,
      isInline: false,
      downloaded: false,
      bytes: 'PDF',
    },
    {
      id: `${base}:att-png`,
      filename: 'logo.png',
      mime: 'image/png',
      size: 4_096,
      isInline: true,
      downloaded: true,
      bytes: 'PNG',
    },
    {
      id: `${base}:att-txt`,
      filename: 'notes.txt',
      mime: 'text/plain',
      size: 512,
      isInline: false,
      downloaded: false,
      bytes: 'TXT',
    },
    {
      // The sender left the filename off a real part. It is still an
      // attachment: it belongs in the strip and Save All must not skip it
      // (P2.6).
      id: `${base}:att-unnamed`,
      filename: null,
      mime: 'application/octet-stream',
      size: 2_048,
      isInline: false,
      downloaded: false,
      bytes: 'BIN',
    },
  ];
}

function longMessageBody(n: number): { html: string; text: string } {
  // Message numbers 200…197 are the four newest, which the reader expands by
  // default: RTL, CJK, no HTML part, and no body at all.
  switch (n) {
    case 200:
      // Right-to-left: the reader must honour the message's own direction.
      return {
        html: '<p dir="rtl" lang="ar">رسالة عربية طويلة لاختبار الاتجاه من اليمين إلى اليسار.</p>',
        text: 'رسالة عربية طويلة لاختبار الاتجاه من اليمين إلى اليسار.',
      };
    case 199:
      return {
        html: '<p lang="ja">日本語のメッセージ本文です。</p>',
        text: '日本語のメッセージ本文です。',
      };
    // No HTML part: the reader renders text only rather than an empty frame.
    case 198:
      return { html: '', text: 'Plain-only body with no HTML part.' };
    case 197:
      return { html: '', text: '' };
    // Distinct per message, so a mis-indexed render cannot pass by accident.
    default:
      return { html: `<p>Long thread message ${n}.</p>`, text: `Long thread message ${n}` };
  }
}

/**
 * The reader's long conversation (P9.2), in the order the reader renders it:
 * `long-0-m1` … `long-0-m200`, one minute apart, senders alternating.
 */
function longThreadConversation(spec: ThreadSpec): FixtureMessage[] {
  const conversation: FixtureMessage[] = [];
  for (let n = 1; n <= LONG_THREAD_MESSAGES; n++) {
    const body = longMessageBody(n);
    conversation.push({
      id: `${spec.id}-m${n}`,
      threadId: spec.id,
      accountId: spec.accountId,
      internalDate: spec.lastMessageAt + n * 60_000,
      from: n % 2 === 1 ? ADA : BEN,
      to: [addr(spec.accountId === ACCOUNT_A ? ADA.e : BEN.e)],
      cc: [],
      bcc: [],
      subject: spec.subject,
      snippet: spec.snippet,
      isUnread: false,
      isStarred: false,
      isDraft: false,
      isSentByMe: false,
      labelIds: ['INBOX'],
      hasAttachments: false,
      attachments: [],
      bodyState: 'fetched',
      html: body.html,
      text: body.text,
    });
  }
  return conversation;
}

/** Every message a spec describes, in ascending `internalDate` order. */
function conversationFor(spec: ThreadSpec): FixtureMessage[] {
  const messageId = `${spec.id}-m1`;
  const attachments: AttachmentMeta[] = (
    spec.id === 'blue-00' ? attachmentSet(spec.accountId, messageId) : []
  ).map((a) => ({
    id: a.id,
    filename: a.filename,
    mime: a.mime,
    size: a.size,
    isInline: a.isInline,
    downloaded: a.downloaded,
  }));
  const bodyHtml =
    spec.blocksHtml ??
    `<p>${spec.subject} — body for ${spec.id}.</p>` +
      (spec.id === 'blue-00' ? '<script>window.__sift_xss=1</script>' : '');
  const first: FixtureMessage = {
    id: messageId,
    threadId: spec.id,
    accountId: spec.accountId,
    internalDate: spec.lastMessageAt,
    from: addr(spec.from, spec.fromName),
    to: [addr(spec.accountId === ACCOUNT_A ? 'ada@example.test' : 'ben@example.test')],
    cc: [],
    bcc: [],
    subject: spec.subject,
    snippet: spec.snippet,
    isUnread: spec.unread,
    isStarred: !!spec.starred,
    isDraft: spec.labelIds.includes('DRAFT'),
    isSentByMe: spec.labelIds.includes('SENT'),
    labelIds: [...spec.labelIds],
    hasAttachments: attachments.length > 0,
    attachments,
    bodyState: 'fetched',
    html: bodyHtml,
    text: `${spec.subject} — body text`,
    ...(spec.id === 'mixed-04'
      ? { listUnsubscribe: { url: 'https://lists.example.test/u', oneClick: true } }
      : {}),
  };
  const conversation = [first];
  for (const [i, extra] of (spec.extra ?? []).entries()) {
    conversation.push({
      ...first,
      id: `${spec.id}-m${i + 2}`,
      internalDate: spec.lastMessageAt + (i + 1) * 60_000,
      from: extra.from,
      isUnread: extra.unread,
      isStarred: false,
      isDraft: false,
      isSentByMe: false,
      labelIds: [...extra.labelIds],
      hasAttachments: false,
      attachments: [],
      html: `<p>${spec.subject} — later message.</p>`,
      text: `${spec.subject} — later message`,
      listUnsubscribe: undefined,
    });
  }
  return conversation;
}

export function seedDatabase(scenario: Scenario = 'default'): FixtureDb {
  const accounts =
    scenario === 'empty'
      ? []
      : [
          account(ACCOUNT_A, 'ada@example.test', 'Ada Lovelace', 'blue', 0),
          account(ACCOUNT_B, 'ben@example.test', 'Ben Ortiz', 'rose', 1),
        ];

  const threads: FixtureThread[] = [];
  const messages: FixtureMessage[] = [];

  for (const spec of threadSpecs()) {
    if (scenario === 'empty') break;
    const conversation = spec.id === LONG_THREAD_ID ? longThreadConversation(spec) : conversationFor(spec);
    messages.push(...conversation);
    // Participants are the senders in date order, as the Rust aggregate builds
    // them — so a conversation with a reply names the reply's sender too.
    const participants: Address[] = [];
    for (const m of conversation) {
      if (!participants.some((p) => p.e === m.from.e)) participants.push(m.from);
    }
    threads.push({
      accountId: spec.accountId,
      id: spec.id,
      subject: spec.subject,
      snippet: spec.snippet,
      participants,
      lastMessageAt: conversation[conversation.length - 1].internalDate,
      messageCount: conversation.length,
      unreadCount: conversation.filter((m) => m.isUnread).length,
      isStarred: !!spec.starred,
      hasAttachments: conversation.some((m) => m.hasAttachments),
      labelIds: [...new Set(conversation.flatMap((m) => m.labelIds))],
      ...(spec.id === 'snoozed-0' ? { snoozedUntil: FIXED_NOW + 86_400_000 } : {}),
      searchText: `${spec.subject} ${spec.snippet} ${conversation
        .map((m) => `${m.from.e} ${m.from.n ?? ''}`)
        .join(' ')}`.toLowerCase(),
    });
  }

  const perAccount = (id: string) => threads.filter((t) => t.accountId === id);
  const inboxUnread = (id: string) =>
    perAccount(id).filter((t) => t.labelIds.includes('INBOX') && t.unreadCount > 0).length;
  const count = (id: string, l: string) => perAccount(id).filter((t) => t.labelIds.includes(l)).length;

  const labels: Label[] =
    scenario === 'empty'
      ? []
      : [
          label(ACCOUNT_A, 'INBOX', 'Inbox', 'system', inboxUnread(ACCOUNT_A), count(ACCOUNT_A, 'INBOX'), 0),
          label(ACCOUNT_A, 'STARRED', 'Starred', 'system', 0, count(ACCOUNT_A, 'STARRED') || 2, 1),
          label(ACCOUNT_A, 'SENT', 'Sent', 'system', 0, count(ACCOUNT_A, 'SENT'), 2),
          label(ACCOUNT_A, 'DRAFT', 'Drafts', 'system', 0, count(ACCOUNT_A, 'DRAFT'), 3),
          label(ACCOUNT_A, 'ARCHIVE', 'Archive', 'system', 0, count(ACCOUNT_A, 'ARCHIVE'), 4),
          label(ACCOUNT_A, 'SPAM', 'Spam', 'system', count(ACCOUNT_A, 'SPAM'), count(ACCOUNT_A, 'SPAM'), 5),
          label(ACCOUNT_A, 'TRASH', 'Trash', 'system', 0, count(ACCOUNT_A, 'TRASH'), 6),
          label(ACCOUNT_A, 'Label_A_Client', 'Client Work', 'user', 1, 3, 7),
          label(ACCOUNT_A, 'Label_A_Receipts', 'Receipts', 'user', 0, 3, 8),
          label(ACCOUNT_B, 'INBOX', 'Inbox', 'system', inboxUnread(ACCOUNT_B), count(ACCOUNT_B, 'INBOX'), 0),
          label(ACCOUNT_B, 'STARRED', 'Starred', 'system', 0, 0, 1),
          label(ACCOUNT_B, 'SENT', 'Sent', 'system', 0, 0, 2),
          label(ACCOUNT_B, 'DRAFT', 'Drafts', 'system', 0, 0, 3),
          label(ACCOUNT_B, 'ARCHIVE', 'Archive', 'system', 0, count(ACCOUNT_B, 'ARCHIVE'), 4),
          label(ACCOUNT_B, 'SPAM', 'Spam', 'system', 0, 0, 5),
          label(ACCOUNT_B, 'TRASH', 'Trash', 'system', 0, count(ACCOUNT_B, 'TRASH'), 6),
          label(ACCOUNT_B, 'Label_B_Client', 'Client Work', 'user', 0, 2, 7),
        ];

  const sync: SyncStatus[] =
    scenario === 'empty'
      ? []
      : [
          { account_id: ACCOUNT_A, phase: 'idle', done: 400, total: 400, last_error: null },
          { account_id: ACCOUNT_B, phase: 'idle', done: 400, total: 400, last_error: null },
        ];

  return {
    version: 1,
    scenario,
    accounts,
    labels,
    threads,
    messages,
    drafts: [],
    outbox: [],
    settings: { ...defaultSettings },
    sync,
    nextOpId: 1,
    reminders: [],
    rules: [],
    vips: {},
    // Notifications start enabled and granted so a test has to opt into the
    // denial path rather than silently covering it (P8.4).
    notifications: {
      enabled: true,
      permission: 'granted',
      filter: 'inbox',
      hideSubject: false,
      sound: 'native',
      accountIds: [ACCOUNT_A, ACCOUNT_B],
    },
    nextRuleId: 1,
  };
}

export const FIXTURE_DB_KEY = 'sift-e2e-db-v1';
