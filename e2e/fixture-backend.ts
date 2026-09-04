// TypeScript fixture backend wired through mockIPC: implements every command over in-memory dataset.
import type { ThreadRow, ThreadsPage, View } from '../src/app/ipc/types';

export interface FixtureThread extends ThreadRow {
  bodyHtml?: string;
}

let threads: FixtureThread[] = [];
let listeners: Record<string, ((p: unknown) => void)[]> = {};
let accounts = [
  {
    id: 'a1',
    email: 'ada@x.com',
    display_name: 'Ada',
    color: 'blue',
    provider: 'gmail',
    sync_state: 'partial',
    created_at: 0,
    sort_order: 0,
  },
  {
    id: 'a2',
    email: 'ben@y.org',
    display_name: 'Ben',
    color: 'rose',
    provider: 'gmail',
    sync_state: 'partial',
    created_at: 0,
    sort_order: 1,
  },
];

export function seed(n = 200) {
  threads = [];
  for (let i = 0; i < n; i++) {
    const acc = i % 2 === 0 ? 'a1' : 'a2';
    threads.push({
      accountId: acc,
      id: `t${i}`,
      subject: `Subject ${i}`,
      snippet: `Snippet ${i}`,
      participants: [{ e: 'ada@x.com', n: 'Ada' }],
      lastMessageAt: Date.now() - i * 60000,
      messageCount: 1,
      unreadCount: i % 3 === 0 ? 1 : 0,
      isStarred: i % 7 === 0,
      hasAttachments: i % 5 === 0,
      labelIds: i % 4 === 0 ? [] : ['INBOX'],
      bodyHtml: `<p>Hello ${i}<script>window.__sift_xss=1</script></p>`,
    });
  }
}

export function emit(event: string, payload: unknown) {
  (listeners[event] ?? []).forEach((h) => h(payload));
}

export function onFixture(event: string, h: (p: unknown) => void) {
  (listeners[event] ??= []).push(h);
}

function viewFilter(t: FixtureThread, view: View): boolean {
  switch (view.kind) {
    case 'inbox':
      return t.labelIds.includes('INBOX');
    case 'starred':
      return t.isStarred;
    case 'archive':
      return !t.labelIds.includes('INBOX');
    case 'label':
      return t.labelIds.includes((view as { labelId: string }).labelId);
    case 'search': {
      const q = (view as { q: string }).q.toLowerCase();
      return t.subject.toLowerCase().includes(q) || t.snippet.toLowerCase().includes(q);
    }
    default:
      return true;
  }
}

export const fixtureBackend: Record<string, (args: never) => unknown> = {
  accounts_list: () => accounts,
  accounts_add_google: () => accounts[0],
  accounts_remove: () => null,
  threads_query: ((args: { query: { accountIds: string[]; view: View; limit: number } }) => {
    const { accountIds, view, limit } = args.query;
    const rows = threads
      .filter((t) => accountIds.includes(t.accountId) && viewFilter(t, view))
      .slice(0, limit);
    const page: ThreadsPage = { rows, generation: 1 };
    return page;
  }) as never,
  thread_get: ((args: { accountId: string; threadId: string }) => {
    const t = threads.find((x) => x.id === args.threadId);
    return {
      accountId: args.accountId,
      id: args.threadId,
      subject: t?.subject ?? '',
      labelIds: [],
      messages: [],
    };
  }) as never,
  message_body: ((args: { messageId: string }) => ({
    messageId: args.messageId,
    state: 'ready',
    html: '<p>hi</p>',
    remoteImageCount: 0,
    trackerCount: 0,
    darkSafe: true,
    remoteImagesAllowed: false,
  })) as never,
  threads_action: () => ({ undo_group: 'g1' }),
  action_undo: () => null,
  search: ((args: { q: string }) => {
    const rows = threads.filter((t) => t.subject.toLowerCase().includes(args.q.toLowerCase())).slice(0, 100);
    return { rows, generation: 1 };
  }) as never,
  settings_get: () => ({ theme: 'system', accent: 'blue', density: 'default', readingPane: 'right' }),
  settings_set: (args: never) => (args as { patch: unknown }).patch,
};

export function installMockIPC(page: { addInitScript: (s: { content: string }) => Promise<void> }) {
  void page;
}
