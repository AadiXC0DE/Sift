/**
 * Phase 8 command surface (R8 owns the Rust side and the shared DTO barrel).
 *
 * These wrappers exist so the feature code never guesses a wire shape: every
 * name and field here mirrors the authoritative contract in
 * `local://ipc-p8-contract.md`. They call the same `call()` transport as
 * `src/app/ipc/commands.ts` rather than growing that module, so the shared
 * fixtures and the DTO-sync gate keep one owner.
 *
 * Nothing in this module emits an OS notification, opens a URL or sends mail:
 * it only moves the user's own state between the UI and the backend.
 */
import { call } from '../../app/ipc/commands';
import type { Contact, GestureResponse, Label, StorageUsage } from '../../app/ipc/types';

export interface MessageTarget {
  accountId: string;
  threadId: string;
}

/** One message reminder. It never changes labels or a mail timestamp (P8.2). */
export interface ReminderRow {
  accountId: string;
  threadId: string;
  remindAt: number;
  completedAt: number | null;
  deliveredAt: number | null;
  /** `scheduled` | `due` (no notification was ever confirmed) | `delivered` | `completed`. */
  state: 'scheduled' | 'due' | 'delivered' | 'completed';
  subject: string | null;
  fromName: string | null;
  unread: boolean;
}

export type RuleField = 'sender' | 'recipient' | 'subject' | 'hasAttachment';
export type RuleOp = 'contains' | 'is' | 'domain' | 'isTrue';
export type RuleActionKind = 'addLabel' | 'archive' | 'markRead' | 'star' | 'junk';

export interface RuleCondition {
  field: RuleField;
  op: RuleOp;
  value: string;
}

export interface RuleAction {
  kind: RuleActionKind;
  labelId: string | null;
}

export interface MailRule {
  id: string;
  accountId: string;
  name: string;
  enabled: boolean;
  match: 'all' | 'any';
  conditions: RuleCondition[];
  actions: RuleAction[];
  sortOrder: number;
  revision: number;
  lastError: string | null;
}

/** The fields `rules_upsert` accepts; `id` absent means "create". */
export interface MailRuleInput {
  id?: string | null;
  accountId: string;
  name: string;
  enabled: boolean;
  match: 'all' | 'any';
  conditions: RuleCondition[];
  actions: RuleAction[];
  sortOrder?: number;
}

export interface RulePreviewRow {
  threadId: string;
  subject: string;
  fromName: string | null;
  fromEmail: string;
  wouldJunk: boolean;
}

export interface RulePreview {
  count: number;
  sample: RulePreviewRow[];
}

export interface NotificationsState {
  enabled: boolean;
  /** `unsupported` means this build/OS cannot deliver; the UI must say so. */
  permission: 'granted' | 'denied' | 'prompt' | 'unsupported';
  filter: 'off' | 'inbox' | 'vip';
  hideSubject: boolean;
  sound: 'none' | 'native';
  accountIds: string[];
}

/** A `mailto:` the OS handed the app; never auto-sent (P9.3). */
export interface PendingMailto {
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  body: string;
}

/** `drafts_send`/`send_reschedule`/`send_now` all answer with this (P8.1). */
export interface ScheduleHandle {
  opId: number;
  notBefore: number;
  scheduledAt?: number | null;
  scheduledLocalTime?: string | null;
  scheduledTimezone?: string | null;
}

/**
 * The schedule half of a send handle, read through an optional shape so a
 * caller compiles whether or not the shared DTO barrel has caught up with the
 * contract yet (it gains these fields in the same change).
 */
export interface ScheduleHandleFields {
  scheduledAt?: number | null;
  scheduledLocalTime?: string | null;
  scheduledTimezone?: string | null;
}

export const utilities = {
  // --- P8.1 Send Later -----------------------------------------------------
  drafts_send: (a: { localId: string; revision: number; notBefore?: number; archiveAfterSend?: boolean }) =>
    call<ScheduleHandle>('drafts_send', a),
  send_reschedule: (a: {
    opId: number;
    notBefore: number;
    scheduledLocalTime: string;
    scheduledTimezone: string;
  }) => call<ScheduleHandle>('send_reschedule', a),
  send_now: (a: { opId: number }) => call<ScheduleHandle>('send_now', a),

  // --- P8.2 Reminders ------------------------------------------------------
  reminder_set: (a: { targets: MessageTarget[]; remindAt: number }) => call<ReminderRow[]>('reminder_set', a),
  reminder_clear: (a: { targets: MessageTarget[] }) => call<ReminderRow[]>('reminder_clear', a),
  reminders_list: (a: { accountIds: string[]; includeCompleted?: boolean }) =>
    call<ReminderRow[]>('reminders_list', a),

  // --- P8.3 Rules ----------------------------------------------------------
  rules_list: (a: { accountId: string }) => call<MailRule[]>('rules_list', a),
  rules_upsert: (a: { rule: MailRuleInput; expectedRevision?: number }) => call<MailRule>('rules_upsert', a),
  rules_delete: (a: { accountId: string; ruleId: string }) => call<void>('rules_delete', a),
  rules_preview: (a: { accountId: string; rule: MailRuleInput; limit?: number }) =>
    call<RulePreview>('rules_preview', a),
  rules_apply_existing: (a: { accountId: string; ruleId: string; revision: number }) =>
    call<{ applied: number; skipped: number }>('rules_apply_existing', a),
  rule_block_sender: (a: { accountId: string; email: string; name?: string }) =>
    call<MailRule>('rule_block_sender', a),

  // --- P8.4 Notifications / VIPs -------------------------------------------
  notifications_state: () => call<NotificationsState>('notifications_state'),
  notifications_enable: (a: { enabled: boolean }) => call<NotificationsState>('notifications_enable', a),
  notifications_update: (a: {
    filter?: NotificationsState['filter'];
    hideSubject?: boolean;
    sound?: NotificationsState['sound'];
    accountIds?: string[];
  }) => call<NotificationsState>('notifications_update', a),
  vip_list: (a: { accountId: string }) => call<string[]>('vip_list', a),
  vip_set: (a: { accountId: string; email: string; vip: boolean }) => call<string[]>('vip_set', a),
  vip_candidates: (a: { accountId: string; limit?: number }) => call<Contact[]>('vip_candidates', a),

  // --- P8.5 Labels and message management ----------------------------------
  label_rename: (a: { accountId: string; labelId: string; name: string }) => call<Label>('label_rename', a),
  label_delete: (a: { accountId: string; labelId: string }) => call<void>('label_delete', a),
  trash_empty_preview: (a: { accountIds: string[] }) => call<{ count: number }>('trash_empty_preview', a),
  trash_empty: (a: { accountIds: string[] }) => call<GestureResponse>('trash_empty', a),

  // --- P9.3 Native utilities ----------------------------------------------
  message_raw_export: (a: { accountId: string; messageId: string }) =>
    call<{ path: string | null }>('message_raw_export', a),
  mailto_pending: () => call<PendingMailto | null>('mailto_pending'),
  mailto_take: () => call<PendingMailto | null>('mailto_take'),

  // --- P10.4 Storage cap ---------------------------------------------------
  storage_set_attachment_cap: (a: { bytes: number }) => call<StorageUsage>('storage_set_attachment_cap', a),
};

/**
 * `ThreadRow`/`Label` gain fields in the same change; reading them through
 * these optional shapes means the feature code compiles before and after the
 * shared DTO barrel picks them up, and never invents a value that is absent.
 */
export interface ThreadRowWithReminder {
  reminderAt?: number | null;
}

export interface LabelWithParent {
  parentId?: string | null;
  depth?: number;
}
