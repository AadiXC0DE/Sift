export interface Command {
  id: string;
  title: string;
  section: string;
  shortcut?: string;
  when: () => boolean;
  run: () => void | Promise<void>;
}

const regs: Command[] = [];

export function registerCommand(c: Command): void {
  regs.push(c);
}

export function allCommands(): Command[] {
  return regs.filter((c) => {
    try {
      return c.when();
    } catch {
      return false;
    }
  });
}

export function filterCommands(q: string): Command[] {
  const query = q.toLowerCase();
  const cmds = allCommands();
  if (!query) return cmds;
  // fuzzy: subsequence score, prefix bonus
  return cmds
    .map((c) => ({ c, s: score(c.title.toLowerCase(), query) }))
    .filter((x) => x.s > 0)
    .sort((a, b) => b.s - a.s)
    .map((x) => x.c);
}

function score(title: string, q: string): number {
  if (title === q) return 1000;
  if (title.startsWith(q)) return 500 + q.length;
  // subsequence
  let ti = 0,
    qi = 0,
    s = 0;
  while (ti < title.length && qi < q.length) {
    if (title[ti] === q[qi]) {
      s += 10 - Math.min(9, ti - qi);
      qi++;
    }
    ti++;
  }
  return qi === q.length ? s : 0;
}

// Default registrations (features add more at import time)
import { useView } from '../../stores/viewStore';
import { undoLast } from '../actions/dispatch';
import { api } from '../../app/ipc/commands';

registerCommand({
  id: 'go-inbox',
  title: 'Go to Inbox',
  section: 'Go to',
  shortcut: 'g i',
  when: () => true,
  run: () => useView.getState().setView({ kind: 'inbox' }),
});
registerCommand({
  id: 'go-starred',
  title: 'Go to Starred',
  section: 'Go to',
  shortcut: 'g s',
  when: () => true,
  run: () => useView.getState().setView({ kind: 'starred' }),
});
registerCommand({
  id: 'go-snoozed',
  title: 'Go to Snoozed',
  section: 'Go to',
  when: () => true,
  run: () => useView.getState().setView({ kind: 'snoozed' }),
});
registerCommand({
  id: 'go-sent',
  title: 'Go to Sent',
  section: 'Go to',
  when: () => true,
  run: () => useView.getState().setView({ kind: 'sent' }),
});
registerCommand({
  id: 'go-archive',
  title: 'Go to Archive',
  section: 'Go to',
  when: () => true,
  run: () => useView.getState().setView({ kind: 'archive' }),
});
registerCommand({
  id: 'compose',
  title: 'Compose',
  section: 'Compose',
  shortcut: 'c',
  when: () => true,
  run: () => {
    document.dispatchEvent(new CustomEvent('sift:compose'));
  },
});
registerCommand({
  id: 'sync',
  title: 'Sync now',
  section: 'General',
  shortcut: '⌘R',
  when: () => true,
  run: () => void api.sync_now(),
});
registerCommand({
  id: 'undo',
  title: 'Undo last action',
  section: 'General',
  shortcut: 'z',
  when: () => true,
  run: () => void undoLast(),
});
registerCommand({
  id: 'settings',
  title: 'Open Settings',
  section: 'General',
  shortcut: '⌘,',
  when: () => true,
  run: () => {
    document.dispatchEvent(new CustomEvent('sift:settings'));
  },
});
registerCommand({
  id: 'diag',
  title: 'Export diagnostics',
  section: 'General',
  when: () => true,
  run: () => void api.diagnostics_export(),
});
