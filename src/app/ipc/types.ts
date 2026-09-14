// DTO mirror of src-tauri/src/dto.rs - check-dto-sync.ts enforces field parity.
export interface Account {
  id: string;
  provider: string;
  email: string;
  display_name?: string | null;
  avatar_url?: string | null;
  color: string;
  auth_kind: string;
  history_id?: string | null;
  sync_state: string;
  last_sync_at?: number | null;
  created_at: number;
  sort_order: number;
  signature_html?: string | null;
  /** Rust `granted_scope`: what Google actually granted; gates permanent deletion. */
  grantedScope?: string | null;
}

/**
 * Attachment DTOs that are Rust-internal (never serialized across the IPC
 * boundary) keep their Rust field names here so the DTO-sync gate can see the
 * contract explicitly: `part_id`, `gmail_att_id`, `content_id`, `is_inline`,
 * `data_error`, `local_path`, `cache_state`, `decoded_size` on
 * `dto::AttachmentRecord`, and the `CacheState` values it carries.
 */
export interface AttachmentRecordFields {
  part_id: string;
  gmail_att_id?: string | null;
  content_id?: string | null;
  is_inline: boolean;
  data_error?: string | null;
  local_path?: string | null;
  cache_state: 'missing' | 'metadata' | 'ready' | 'unverified';
  decoded_size?: number | null;
}
export interface SyncStatus {
  account_id: string;
  phase: string;
  done: number;
  total: number;
  /**
   * Whether `total` is a real message count (P4.6). `false`/absent means the
   * mailbox is still being listed: show indeterminate progress, never
   * "mailbox empty". The backend always sends it; it is optional here so
   * test fixtures that predate it read as "unknown" instead of lying.
   */
  totalKnown?: boolean;
  last_error?: string | null;
}

/**
 * Native connectivity for one account (P4.6). The host reachability hint is
 * only a hint; `state` is backed by the outcome of real provider operations.
 */
export interface ConnectivityState {
  accountId: string;
  state: 'online' | 'offline' | 'degraded' | 'reauth_required';
  /** Unix ms of the last successful provider operation, if any. */
  lastOkAt?: number | null;
  lastError?: string | null;
  /** Unix ms when the account entered this state. */
  since: number;
}
export interface Label {
  account_id: string;
  id: string;
  name: string;
  kind: string;
  color_bg?: string | null;
  color_fg?: string | null;
  visible: boolean;
  unread_count: number;
  total_count: number;
  sort_order: number;
}
export type View =
  | { kind: 'inbox' }
  | { kind: 'starred' }
  | { kind: 'snoozed' }
  | { kind: 'sent' }
  | { kind: 'drafts' }
  | { kind: 'archive' }
  /**
   * All Mail (P3.6): every thread with at least one message outside
   * Trash/Junk. A MAILBOX scope, not an account scope — it never maps onto
   * the unified "all accounts" selection; `accountIds` still filters rows.
   */
  | { kind: 'all_mail' }
  | { kind: 'spam' }
  | { kind: 'trash' }
  | { kind: 'label'; labelId: string }
  | { kind: 'search'; q: string };
/**
 * Account-qualified thread identity (P3.2). Provider thread IDs are only
 * unique within an account, so every navigation/action target carries the pair.
 */
export type ThreadRef = { accountId: string; threadId: string };
/**
 * Account-qualified message identity (P4.2). Provider message IDs are only
 * unique within an account, so every read/mutation target carries the pair.
 */
export type MessageRef = { accountId: string; messageId: string };
/** Account-qualified attachment identity (P4.2). */
export type AttachmentRefKey = { accountId: string; attachmentId: string };
/** Result of `attachments_save_as`; a cancelled dialog is a normal outcome. */
export interface SaveAsResult {
  path?: string | null;
  cancelled: boolean;
}
/** Result of a message-level Save All. */
export interface SaveAllResult {
  saved: number;
  failed: AttachmentRefKey[];
}
export interface ThreadsQuery {
  accountIds: string[];
  view: View;
  cursor?: string;
  limit: number;
  unread_only?: boolean;
  has_attachment?: boolean;
}
export interface Address {
  n?: string;
  e: string;
  me?: boolean;
}
export interface ThreadRow {
  accountId: string;
  id: string;
  subject: string;
  snippet: string;
  participants: Address[];
  lastMessageAt: number;
  messageCount: number;
  unreadCount: number;
  isStarred: boolean;
  hasAttachments: boolean;
  labelIds: string[];
  snoozedUntil?: number;
  serverOnly?: boolean;
}
export interface ThreadsPage {
  rows: ThreadRow[];
  nextCursor?: string;
  total?: number;
  generation: number;
}

/**
 * One recoverable problem with a query (P7.1). `kind` is stable: an invalid
 * date or over-nested group makes the query unmatchable, an unknown operator
 * is preserved as literal text and reported here so the UI can offer
 * "Search Gmail for full syntax".
 */
export interface QueryHint {
  kind: string;
  message: string;
  token?: string;
}

/**
 * One page of search results (P7.1/P7.3). A superset of `ThreadsPage`, so a
 * caller that only reads rows keeps working.
 */
export interface SearchPage {
  rows: ThreadRow[];
  nextCursor?: string;
  total?: number;
  generation: number;
  hints: QueryHint[];
  query: string;
  /** Accounts whose server search failed while others succeeded. */
  failedAccounts: string[];
  /** The result is limited to downloaded mail. */
  localOnly: boolean;
}

/**
 * A saved search (P7.4): a validated query plus an account scope. It never
 * holds a copy of any message.
 */
export interface SavedSearch {
  id: string;
  name: string;
  query: string;
  /** An empty scope matches nothing; a scope is never widened to all accounts. */
  accountScope: string[];
  /** Scope entries whose account no longer exists, offered for editing. */
  missingAccounts: string[];
  sortOrder: number;
  createdAt: number;
  /** The AST version the stored query was validated against. */
  astVersion: number;
  /** Lazy count: absent until the mailbox is visible and asks for one. */
  matchCount?: number;
  countedAt?: number;
}

export interface SavedSearchInput {
  id?: string;
  name: string;
  query: string;
  accountScope: string[];
  sortOrder?: number;
}

export interface SavedSearchCount {
  id: string;
  count: number;
  computedAt: number;
}
export interface AttachmentMeta {
  id: string;
  filename?: string | null;
  mime: string;
  size: number;
  isInline: boolean;
  downloaded: boolean;
}
export interface ListUnsub {
  url?: string;
  mailto?: string;
  oneClick: boolean;
  /** Every structured header entry, in header order (P9.4). Optional only so
   * pre-P9.4 fixtures keep typechecking. */
  targets?: UnsubscribeTarget[];
}

/** One parsed `List-Unsubscribe` entry (P9.4). */
export interface UnsubscribeTarget {
  /** `https` | `http` | `mailto` | `other`. */
  scheme: string;
  url: string;
}

/**
 * Outcome of one user-initiated unsubscribe (P9.4). When the fields say Sift
 * will not POST on the user's behalf, `reason`/`detail` explain why and `url`
 * or `mailto` is the labelled fallback.
 */
export interface UnsubscribeResult {
  /** `one_click` | `open_link` | `mailto` | `none`. */
  method: string;
  done: boolean;
  url?: string;
  mailto?: string;
  reason?: string;
  status?: number;
  detail?: string;
  targets: UnsubscribeTarget[];
}
export interface MessageMeta {
  id: string;
  internalDate: number;
  from: Address;
  to: Address[];
  cc: Address[];
  bcc: Address[];
  replyTo?: string;
  subject: string;
  snippet: string;
  isUnread: boolean;
  isStarred: boolean;
  isDraft: boolean;
  isSentByMe: boolean;
  labelIds: string[];
  hasAttachments: boolean;
  attachments: AttachmentMeta[];
  bodyState: 'none' | 'fetched' | 'error';
  listUnsubscribe?: ListUnsub;
}
export interface ThreadDetail {
  accountId: string;
  id: string;
  subject: string;
  labelIds: string[];
  messages: MessageMeta[];
}
export interface MessageBody {
  messageId: string;
  state: 'ready' | 'loading' | 'error';
  html?: string;
  text?: string;
  remoteImageCount: number;
  trackerCount: number;
  darkSafe: boolean;
  /** What the backend actually did for this read; never a policy request. */
  remoteImagesAllowed: boolean;
  /**
   * The effective policy used: `block` | `ask` | `allow` (P9.1). The backend
   * always sends these four; they are optional here only so fixtures written
   * before the policy existed still typecheck.
   */
  remoteContentMode?: string;
  /** Bumped whenever this account's effective permission changes. */
  privacyGeneration?: number;
  /** Bumped when the sanitizer's output for the same input changes. */
  renderVersion?: number;
  /** Inline references that could not be resolved from cached parts. */
  unresolvedInlineCount?: number;
}

/**
 * The visible remote-content policy and the state the privacy panel needs
 * (P9.1). `choicePending` is true while an upgraded database still has to
 * answer the one-time compact privacy choice.
 */
export interface RemoteContentPolicy {
  mode: string;
  choicePending: boolean;
  allowedSenders: string[];
  generation: number;
  privacyNotice: string;
}
/** Result of `attachments_open` (Rust `display_name`, `cache_basename`,
 * `decoded_size`, `requires_confirmation`). */
export interface AttachmentOpenResult {
  accountId: string;
  attachmentId: string;
  state: string;
  path?: string | null;
  displayName?: string | null;
  cacheBasename: string;
  mime: string;
  size: number;
  decodedSize?: number | null;
  requiresConfirmation: boolean;
}

/** One attachment transfer state record (Rust `request_id`,
 * `transferred_bytes`). */
export interface AttachmentProgress {
  accountId: string;
  attachmentId: string;
  requestId: string;
  state: 'queued' | 'downloading' | 'ready' | 'failed' | 'cancelled';
  transferredBytes: number;
  totalBytes?: number | null;
  errorCode?: string | null;
}

export interface AttachmentRef {
  name: string;
  mime: string;
  size: number;
  path: string;
}
export type ActionKind =
  | { kind: 'archive' }
  | { kind: 'unarchive' }
  | { kind: 'trash' }
  | { kind: 'untrash' }
  | { kind: 'spam' }
  | { kind: 'unspam' }
  | { kind: 'deleteForever' }
  | { kind: 'star'; on: boolean }
  | { kind: 'read'; on: boolean }
  | { kind: 'addLabel'; labelId: string }
  | { kind: 'removeLabel'; labelId: string }
  | { kind: 'moveTo'; labelId: string };

export type ThreadAction = {
  accountId: string;
  threadIds: string[];
  action: ActionKind;
};

/** One account-qualified target of a gesture (P6.3). */
export interface GestureTarget {
  accountId: string;
  threadId: string;
}

/** The states of the durable operation machine (P6.1). */
export type OperationState = 'pending' | 'inflight' | 'uncertain' | 'done' | 'failed' | 'cancelled';

/**
 * A lightweight outbox row (P6.6). Raw payloads — which can hold a whole MIME
 * message — are never sent to the list UI; these fields are the durable
 * summary columns written at enqueue.
 */
export interface OutboxOp {
  /** Rust `op_id`. */
  opId: number;
  accountId: string;
  kind: string;
  state: OperationState;
  action: string;
  /** Rust `recipient_summary`. */
  recipientSummary?: string | null;
  subject?: string | null;
  /** Rust `scheduled_at`: when a queued send is due. */
  scheduledAt?: number | null;
  /** Rust `retry_at`: the next attempt or reconciliation. */
  retryAt?: number | null;
  createdAt: number;
  /** Rust `error_code`. */
  errorCode?: string | null;
  errorMessage?: string | null;
  draftId?: string | null;
  revision?: number | null;
  requiresDuplicateAck: boolean;
}

export interface OutboxCounts {
  pending: number;
  inflight: number;
  uncertain: number;
  done: number;
  failed: number;
  cancelled: number;
}

export interface OutboxPage {
  operations: OutboxOp[];
  nextCursor?: string | null;
  total: number;
  counts: OutboxCounts;
}

/** The result of one gesture: its operation ids, per account (Rust
 * `gesture_id`). */
export interface GestureResponse {
  gestureId: string;
  operations: OutboxOp[];
  failures?: { accountId: string; code: string; message: string }[];
}

/**
 * The Rust names behind the camelCase wire fields above, kept explicit for the
 * DTO-sync gate: `op_id`, `error_message`, `draft_id`,
 * `requires_duplicate_ack` on `dto::OutboxOpSummary`; `next_cursor` on
 * `dto::OutboxPage`; `gesture_id`, `targets` on `dto::ThreadsActionRequest`
 * and `dto::SnoozeClearRequest`; `wake_at`, `wake_unread` on
 * `dto::SnoozeSetRequest`.
 */
export type DraftState = 'editing' | 'queued' | 'sent' | 'failed';
export interface Draft {
  localId: string;
  accountId: string;
  fromEmail?: string;
  remoteDraftId?: string;
  remoteMessageId?: string;
  threadId?: string;
  inReplyToMessageId?: string;
  rfcMessageId?: string;
  parentRfcMessageId?: string;
  references?: string[];
  mode: string;
  toJson: Address[];
  ccJson: Address[];
  bccJson: Address[];
  subject: string;
  bodyHtml: string;
  attachmentsJson: AttachmentRef[];
  revision: number;
  savedRevision?: number;
  remoteRevision?: number;
  state: DraftState;
  notBefore?: number;
  scheduledAt?: number;
  scheduledTimezone?: string;
  scheduledLocalTime?: string;
  updatedAt?: number;
}
export interface DraftPage {
  drafts: Draft[];
  nextCursor?: string;
}
export interface SendHandle {
  opId: number;
  notBefore: number;
}
export interface ComposeLimits {
  rawMimeLimitBytes: number;
}
export interface RemoteDraft {
  remoteDraftId: string;
  messageId?: string;
  threadId?: string;
  rfcMessageId?: string;
}
export interface Contact {
  email: string;
  name?: string | null;
  lastUsedAt: number;
  useCount: number;
}
export interface Settings {
  theme: string;
  accent: string;
  uiFont: string;
  density: string;
  readingPane: string;
  avatarsInList: boolean;
  snippetLines: number;
  sidebarLabels: string;
  messageFont: string;
  messageFontSize: number;
  darkModeEmails: string;
  afterArchive: string;
  undoSendDelay: number;
  undoToastDuration: number;
  markAsRead: string;
  replyDefault: string;
  sendAndArchiveDefault: boolean;
  wakeSnoozedUnread: boolean;
  splitInbox: boolean;
  notifications: string;
  sound: string;
  dockBadge: string;
  remoteImages: string;
  /** `block` | `ask` | `allow`; new installations default to `ask` (P9.1). */
  remoteContentMode: string;
  /** True while the one-time upgrade privacy choice is unanswered. */
  remoteContentChoicePending: boolean;
  stripTrackers: boolean;
  offlineBodyCache: string;
  attachmentCacheSize: string;
  pollFocused: number;
  pollBackground: number;
}
export const defaultSettings: Settings = {
  theme: 'system',
  accent: 'blue',
  uiFont: 'system',
  density: 'default',
  readingPane: 'right',
  avatarsInList: true,
  snippetLines: 1,
  sidebarLabels: 'collapse',
  messageFont: 'system',
  messageFontSize: 14,
  darkModeEmails: 'auto',
  afterArchive: 'next',
  undoSendDelay: 10,
  undoToastDuration: 8,
  markAsRead: 'on-open',
  replyDefault: 'reply',
  sendAndArchiveDefault: true,
  wakeSnoozedUnread: true,
  splitInbox: false,
  notifications: 'inbox',
  sound: 'subtle',
  dockBadge: 'unread',
  remoteImages: 'always',
  remoteContentMode: 'ask',
  remoteContentChoicePending: false,
  stripTrackers: true,
  offlineBodyCache: '2y',
  attachmentCacheSize: '512MB',
  pollFocused: 15,
  pollBackground: 60,
};

/** Counts the removal dialog shows before an account is deleted (P4.4). */
export interface RemovalCounts {
  drafts: number;
  queued: number;
  uncertainSends: number;
}

/** One class of app-owned cache; `items` counts rows or files, whichever applies. */
export interface StorageCategory {
  bytes: number;
  items: number;
}

/**
 * Settings -> Storage payload (P10.4). Sizes are exact app-owned bytes, not
 * filesystem allocation, so a clear action can be verified against them.
 */
export interface StorageUsage {
  metadata: StorageCategory;
  bodies: StorageCategory;
  attachments: StorageCategory;
  draftCache: StorageCategory;
  /** Configured cap for `attachments`, in bytes (eviction target, not usage). */
  attachmentCacheLimitBytes: number;
  /** Backend-reported sum of the four categories. */
  totalBytes: number;
  /** Unix ms when the backend measured. */
  computedAt: number;
}

export type SetupProgress =
  'connecting' | 'authenticating' | 'listing' | 'syncing' | 'done' | { error: string };
