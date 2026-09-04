// DTO mirror of src-tauri/src/dto.rs — check-dto-sync.ts enforces field parity.
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
}
export interface SyncStatus {
  account_id: string;
  phase: string;
  done: number;
  total: number;
  last_error?: string | null;
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
  | { kind: 'spam' }
  | { kind: 'trash' }
  | { kind: 'label'; labelId: string }
  | { kind: 'search'; q: string };
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
  remoteImagesAllowed: boolean;
}
export interface AttachmentRef {
  name: string;
  mime: string;
  size: number;
  path: string;
}
export type ThreadAction = {
  accountId: string;
  threadIds: string[];
  action:
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
};
export interface Draft {
  localId?: string;
  accountId: string;
  remoteDraftId?: string;
  remoteMessageId?: string;
  threadId?: string;
  inReplyToMessageId?: string;
  mode: string;
  toJson: Address[];
  ccJson: Address[];
  bccJson: Address[];
  subject: string;
  bodyHtml: string;
  attachmentsJson: AttachmentRef[];
  updatedAt?: number;
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
  remoteImages: 'ask',
  stripTrackers: true,
  offlineBodyCache: '2y',
  attachmentCacheSize: '2GB',
  pollFocused: 15,
  pollBackground: 60,
};
