import { invoke } from '@tauri-apps/api/core';
import { Channel } from '@tauri-apps/api/core';
import type {
  Account,
  AttachmentRef,
  AttachmentRefKey,
  ConnectivityState,
  ComposeLimits,
  Contact,
  Draft,
  DraftPage,
  Label,
  MessageBody,
  RemovalCounts,
  SaveAllResult,
  SaveAsResult,
  SendHandle,
  Settings,
  StorageUsage,
  SyncStatus,
  ThreadAction,
  ThreadDetail,
  ThreadsPage,
  ThreadsQuery,
  SetupProgress,
} from './types';

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const t0 = performance.now();
  try {
    return await invoke<T>(cmd, args as never);
  } finally {
    const dt = performance.now() - t0;
    if (import.meta.env.DEV && dt > 8) console.warn(`[ipc] ${cmd} took ${dt.toFixed(1)}ms`);
  }
}

export const api = {
  accounts_list: () => call<Account[]>('accounts_list'),
  accounts_add_google: (login_hint?: string) =>
    call<Account>('accounts_add_google', { loginHint: login_hint }),
  accounts_probe_email: (email: string) => call<boolean | null>('accounts_probe_email', { email }),
  accounts_add_app_password: (
    email: string,
    appPassword: string,
    onProgress: (p: SetupProgress) => void,
  ) => {
    // new Channel() touches Tauri internals synchronously and throws in a
    // plain browser (e2e, previews). Fall back to no progress channel so the
    // failure surfaces as an async rejection the wizard can render.
    let progress: Channel<SetupProgress> | undefined;
    try {
      progress = new Channel<SetupProgress>();
      progress.onmessage = onProgress;
    } catch {
      progress = undefined;
    }
    return call<Account>('accounts_add_app_password', {
      email,
      appPassword,
      progress,
    });
  },
  accounts_update_app_password: (id: string, appPassword: string) =>
    call<Account>('accounts_update_app_password', { id, appPassword }),
  accounts_remove: (id: string) => call<void>('accounts_remove', { id }),
  accounts_removal_preview: (id: string) =>
    call<RemovalCounts>('accounts_removal_preview', { id }),
  accounts_update: (p: Record<string, unknown>) => call<Account>('accounts_update', p),
  system_info: () => call<{ version: string; oauth_available: boolean; demo: boolean }>('system_info'),
  sync_now: (account_id?: string) => call<void>('sync_now', { accountId: account_id }),
  sync_status: () => call<SyncStatus[]>('sync_status'),
  /** Native per-account connectivity (P4.6). */
  connectivity_state: () => call<ConnectivityState[]>('connectivity_state'),
  /**
   * Host reachability hint (P4.6). The frontend bridges `navigator.onLine`
   * (plus the `online`/`offline` events) here; a hint alone never proves the
   * provider is reachable, so coming back online re-runs one sync tick per
   * account instead of trusting it.
   */
  app_network_hint: (online: boolean) => call<void>('app_network_hint', { online }),
  labels_list: (account_id: string) => call<Label[]>('labels_list', { accountId: account_id }),
  labels_create: (account_id: string, name: string) =>
    call<Label>('labels_create', { accountId: account_id, name }),
  threads_query: (q: ThreadsQuery) => call<ThreadsPage>('threads_query', { query: q }),
  thread_get: (account_id: string, thread_id: string) =>
    call<ThreadDetail>('thread_get', { accountId: account_id, threadId: thread_id }),
  message_body: (account_id: string, message_id: string) =>
    call<MessageBody>('message_body', { accountId: account_id, messageId: message_id }),
  message_raw_source: (account_id: string, message_id: string) =>
    call<string>('message_raw_source', { accountId: account_id, messageId: message_id }),
  threads_action: (a: ThreadAction) => call<{ undo_group: string }>('threads_action', { req: a }),
  action_undo: (undo_group: string) => call<void>('action_undo', { undoGroup: undo_group }),
  snooze_set: (account_id: string, thread_ids: string[], wake_at: number) =>
    call<{ undo_group: string }>('snooze_set', {
      accountId: account_id,
      threadIds: thread_ids,
      wakeAt: wake_at,
    }),
  snooze_clear: (account_id: string, thread_ids: string[]) =>
    call<void>('snooze_clear', { accountId: account_id, threadIds: thread_ids }),
  drafts_get: (a: { localId: string }) => call<Draft>('drafts_get', a),
  drafts_list: (a: { accountIds: string[]; cursor?: string; limit?: number }) =>
    call<DraftPage>('drafts_list', a),
  drafts_upsert: (a: { draft: Draft; expectedRevision?: number }) =>
    call<Draft>('drafts_upsert', a),
  // Explicit discard only: the send path must never call this.
  drafts_delete: (local_id: string) => call<void>('drafts_delete', { localId: local_id }),
  drafts_send: (a: {
    localId: string;
    revision: number;
    notBefore?: number;
    archiveAfterSend?: boolean;
  }) => call<SendHandle>('drafts_send', a),
  send_cancel: (a: { opId: number }) => call<Draft>('send_cancel', a),
  compose_limits: () => call<ComposeLimits>('compose_limits'),
  contacts_suggest: (account_id: string, q: string, limit: number) =>
    call<Contact[]>('contacts_suggest', { accountId: account_id, q, limit }),
  search: (account_ids: string[], q: string, scope: 'local' | 'server') =>
    call<ThreadsPage>('search', { accountIds: account_ids, q, scope }),
  attachments_open: (a: { accountId: string; attachmentId: string; confirmedExecutable?: boolean }) =>
    call<void>('attachments_open', a),
  attachments_save_as: (a: AttachmentRefKey) => call<SaveAsResult>('attachments_save_as', a),
  attachments_save_all: (a: { accountId: string; messageId: string }) =>
    call<SaveAllResult>('attachments_save_all', a),
  attachments_cancel: (a: { accountId: string; requestId: string }) =>
    call<void>('attachments_cancel', a),
  attachments_add_from_paths: (a: { accountId: string; draftId: string; paths: string[] }) =>
    call<AttachmentRef[]>('attachments_add_from_paths', a),
  // Forwarding: stage one attachment of an existing message into a draft.
  attachments_stage_from_message: (a: {
    accountId: string;
    messageId: string;
    attachmentId: string;
    draftId: string;
  }) => call<AttachmentRef>('attachments_stage_from_message', a),
  remote_images_load: (account_id: string, message_id: string, remember_sender: boolean) =>
    call<MessageBody>('remote_images_load', {
      accountId: account_id,
      messageId: message_id,
      rememberSender: remember_sender,
    }),
  settings_get: () => call<Settings>('settings_get'),
  settings_set: (p: Partial<Settings>) => call<Settings>('settings_set', { patch: p }),
  // P10.4 Storage: metering plus a clear action, both returning measured totals.
  storage_usage: () => call<StorageUsage>('storage_usage'),
  storage_clear_attachment_cache: () => call<StorageUsage>('storage_clear_attachment_cache'),
  app_set_badge: (count: number) => call<void>('app_set_badge', { count }),
  app_open_url: (url: string) => call<void>('app_open_url', { url }),
  unsubscribe: (account_id: string, message_id: string) =>
    call<{ method: string; done: boolean }>('unsubscribe', {
      accountId: account_id,
      messageId: message_id,
    }),
  diagnostics_export: () => call<{ path: string }>('diagnostics_export'),
  perf_mark: (name: string) => call<void>('perf_mark', { name }),
};
