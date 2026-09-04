import { invoke } from '@tauri-apps/api/core';
import { Channel } from '@tauri-apps/api/core';

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
  accounts_list: () => call<import('./types').Account[]>('accounts_list'),
  accounts_add_google: (login_hint?: string) =>
    call<import('./types').Account>('accounts_add_google', { loginHint: login_hint }),
  accounts_probe_email: (email: string) => call<boolean | null>('accounts_probe_email', { email }),
  accounts_add_app_password: (
    email: string,
    appPassword: string,
    onProgress: (p: import('./types').SetupProgress) => void,
  ) => {
    const ch = new Channel<import('./types').SetupProgress>();
    ch.onmessage = onProgress;
    return call<import('./types').Account>('accounts_add_app_password', {
      email,
      appPassword,
      progress: ch,
    });
  },
  accounts_update_app_password: (id: string, appPassword: string) =>
    call<import('./types').Account>('accounts_update_app_password', { id, appPassword }),
  accounts_remove: (id: string) => call<void>('accounts_remove', { id }),
  accounts_update: (p: Record<string, unknown>) => call<import('./types').Account>('accounts_update', p),
  system_info: () => call<{ version: string; oauth_available: boolean; demo: boolean }>('system_info'),
  sync_now: (account_id?: string) => call<void>('sync_now', { accountId: account_id }),
  sync_status: () => call<import('./types').SyncStatus[]>('sync_status'),
  labels_list: (account_id: string) =>
    call<import('./types').Label[]>('labels_list', { accountId: account_id }),
  labels_create: (account_id: string, name: string) =>
    call<import('./types').Label>('labels_create', { accountId: account_id, name }),
  threads_query: (q: import('./types').ThreadsQuery) =>
    call<import('./types').ThreadsPage>('threads_query', { query: q }),
  thread_get: (account_id: string, thread_id: string) =>
    call<import('./types').ThreadDetail>('thread_get', { accountId: account_id, threadId: thread_id }),
  message_body: (message_id: string) =>
    call<import('./types').MessageBody>('message_body', { messageId: message_id }),
  message_raw_source: (message_id: string) => call<string>('message_raw_source', { messageId: message_id }),
  threads_action: (a: import('./types').ThreadAction) =>
    call<{ undo_group: string }>('threads_action', { req: a }),
  action_undo: (undo_group: string) => call<void>('action_undo', { undoGroup: undo_group }),
  snooze_set: (account_id: string, thread_ids: string[], wake_at: number) =>
    call<{ undo_group: string }>('snooze_set', {
      accountId: account_id,
      threadIds: thread_ids,
      wakeAt: wake_at,
    }),
  snooze_clear: (account_id: string, thread_ids: string[]) =>
    call<void>('snooze_clear', { accountId: account_id, threadIds: thread_ids }),
  drafts_upsert: (d: import('./types').Draft) => call<import('./types').Draft>('drafts_upsert', { draft: d }),
  drafts_delete: (local_id: string) => call<void>('drafts_delete', { localId: local_id }),
  drafts_send: (local_id: string, undo_delay_ms: number) =>
    call<{ op_id: number }>('drafts_send', { localId: local_id, undoDelayMs: undo_delay_ms }),
  send_cancel: (op_id: number) => call<import('./types').Draft>('send_cancel', { opId: op_id }),
  contacts_suggest: (account_id: string, q: string, limit: number) =>
    call<import('./types').Contact[]>('contacts_suggest', { accountId: account_id, q, limit }),
  search: (account_ids: string[], q: string, scope: 'local' | 'server') =>
    call<import('./types').ThreadsPage>('search', { accountIds: account_ids, q, scope }),
  attachments_open: (attachment_id: string) =>
    call<void>('attachments_open', { attachmentId: attachment_id }),
  attachments_save_as: (attachment_id: string) =>
    call<{ path: string }>('attachments_save_as', { attachmentId: attachment_id }),
  attachments_add_from_paths: (paths: string[]) =>
    call<import('./types').AttachmentRef[]>('attachments_add_from_paths', { paths }),
  remote_images_load: (message_id: string, remember_sender: boolean) =>
    call<import('./types').MessageBody>('remote_images_load', {
      messageId: message_id,
      rememberSender: remember_sender,
    }),
  settings_get: () => call<import('./types').Settings>('settings_get'),
  settings_set: (p: Partial<import('./types').Settings>) =>
    call<import('./types').Settings>('settings_set', { patch: p }),
  app_set_badge: (count: number) => call<void>('app_set_badge', { count }),
  app_open_url: (url: string) => call<void>('app_open_url', { url }),
  unsubscribe: (message_id: string) =>
    call<{ method: string; done: boolean }>('unsubscribe', { messageId: message_id }),
  diagnostics_export: () => call<{ path: string }>('diagnostics_export'),
  perf_mark: (name: string) => call<void>('perf_mark', { name }),
};
