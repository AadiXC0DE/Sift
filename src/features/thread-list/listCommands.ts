import type { ThreadAction, ThreadRow } from '../../app/ipc/types';
import { useSelection } from '../../stores/selectionStore';
import { useView } from '../../stores/viewStore';
import { dispatchGesture } from '../actions/dispatch';
import { registerCommand } from '../palette/registry';
import { rowKey } from './threadWindow';
import { toast } from 'sonner';

export type ListScope = 'list' | 'thread';
export type MailCommand =
  | 'archive'
  | 'trash'
  | 'spam'
  | 'star'
  | 'markRead'
  | 'markUnread'
  /** Not junk: the inverse of `spam` (P8.5). */
  | 'notJunk'
  /** Untrash: moves a conversation out of Trash (P8.5). */
  | 'untrash';
export type ListPickerKind = 'label' | 'move' | 'snooze';

export interface TargetGroup {
  accountId: string;
  threadIds: string[];
}

interface ListContext {
  rows: ThreadRow[];
  openPicker: (kind: ListPickerKind) => void;
}

let context: ListContext | null = null;

/** The mounted list publishes its loaded window for command resolution. */
export function setListContext(next: ListContext): () => void {
  context = next;
  return () => {
    if (context === next) context = null;
  };
}

export function listRows(): ThreadRow[] {
  return context?.rows ?? [];
}

export function hasListContext(): boolean {
  return context !== null && context.rows.length > 0;
}

/**
 * Copy one value for a palette command (P8.5), reporting the outcome: a
 * clipboard that is unavailable must not look like a successful copy.
 */
async function copyToClipboard(value: string, what: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(value);
    toast.success(`${what} copied`);
  } catch {
    toast.error('The clipboard is unavailable');
  }
}

export function groupKeysByAccount(keys: Iterable<string>): TargetGroup[] {
  const byAccount = new Map<string, string[]>();
  for (const key of keys) {
    const i = key.indexOf(':');
    if (i < 0) continue;
    const accountId = key.slice(0, i);
    const threadId = key.slice(i + 1);
    if (!accountId || !threadId) continue;
    const list = byAccount.get(accountId);
    if (list) list.push(threadId);
    else byAccount.set(accountId, [threadId]);
  }
  return [...byAccount].map(([accountId, threadIds]) => ({ accountId, threadIds }));
}

/**
 * The single command target resolver: a non-empty selection wins; otherwise the
 * open reader (thread focus) or the focused list row supplies the target.
 */
export function resolveCommandTargets(scope: ListScope): TargetGroup[] {
  const selection = useSelection.getState().selectedIds;
  if (selection.size > 0) return groupKeysByAccount(selection);

  const rows = listRows();
  // The open reader carries its own account-qualified ref (P3.2); the same
  // thread id can exist in two accounts, so the row lookup is never used to
  // guess the account.
  const open = useView.getState().openThread;
  const openTarget = open ? { accountId: open.accountId, threadIds: [open.threadId] } : null;

  if (scope === 'thread' && openTarget) return [openTarget];

  const focusedKey = useSelection.getState().focusedKey;
  const focused = focusedKey ? rows.find((r) => rowKey(r) === focusedKey) : undefined;
  if (focused) return [{ accountId: focused.accountId, threadIds: [focused.id] }];

  if (openTarget) return [openTarget];
  return [];
}

function primaryRow(targets: TargetGroup[]): ThreadRow | undefined {
  const first = targets[0];
  if (!first) return undefined;
  const rows = listRows();
  return rows.find((r) => r.accountId === first.accountId && first.threadIds.includes(r.id));
}

function actionFor(
  command: MailCommand,
  primary: ThreadRow | undefined,
  overrides: { starOn?: boolean; readOn?: boolean },
): ThreadAction['action'] {
  if (command === 'star') {
    const on = primary ? !primary.isStarred : (overrides.starOn ?? true);
    return { kind: 'star', on };
  }
  if (command === 'markRead' || command === 'markUnread') {
    return { kind: 'read', on: command === 'markRead' };
  }
  // Not junk and untrash are the exact inverses of the two actions the list
  // already performs, so they go through the same gesture + undo path (P8.5).
  if (command === 'notJunk') return { kind: 'unspam' };
  if (command === 'untrash') return { kind: 'untrash' };
  return { kind: command };
}

/**
 * Run a mail command against the resolved targets. A cross-account selection is
 * one gesture with account-qualified targets (P6.3): splitting it into one call
 * per account produced two undo groups for one user action, and undoing it
 * only ever restored one account.
 */
export function runMailCommand(
  command: MailCommand,
  scope: ListScope,
  overrides: { starOn?: boolean; readOn?: boolean } = {},
): boolean {
  const targets = resolveCommandTargets(scope);
  if (!targets.length) return false;
  const primary = primaryRow(targets);
  const action = actionFor(command, primary, overrides);
  void dispatchGesture(
    targets.flatMap((group) => group.threadIds.map((threadId) => ({ accountId: group.accountId, threadId }))),
    action,
  );
  return true;
}

/** Open the list picker when the list owns a selection; false → caller falls back. */
export function openListPicker(kind: ListPickerKind): boolean {
  if (!context) return false;
  context.openPicker(kind);
  return true;
}

// Central registry entries: every configurable list command is discoverable in
// the palette and runs through the same resolver as its key binding.
registerCommand({
  id: 'list-archive',
  title: 'Archive',
  section: 'Actions',
  shortcut: 'e',
  when: hasListContext,
  run: () => void runMailCommand('archive', 'list'),
});
registerCommand({
  id: 'list-trash',
  title: 'Move to Trash',
  section: 'Actions',
  shortcut: '#',
  when: hasListContext,
  run: () => void runMailCommand('trash', 'list'),
});
registerCommand({
  id: 'list-spam',
  title: 'Mark as Spam',
  section: 'Actions',
  shortcut: '!',
  when: hasListContext,
  run: () => void runMailCommand('spam', 'list'),
});
registerCommand({
  id: 'list-star',
  title: 'Toggle Star',
  section: 'Actions',
  shortcut: 's',
  when: hasListContext,
  run: () => void runMailCommand('star', 'list'),
});
registerCommand({
  id: 'list-mark-read',
  title: 'Mark as Read',
  section: 'Actions',
  shortcut: '⇧i',
  when: hasListContext,
  run: () => void runMailCommand('markRead', 'list'),
});
registerCommand({
  id: 'list-mark-unread',
  title: 'Mark as Unread',
  section: 'Actions',
  shortcut: '⇧u',
  when: hasListContext,
  run: () => void runMailCommand('markUnread', 'list'),
});
registerCommand({
  id: 'list-snooze',
  title: 'Snooze',
  section: 'Actions',
  shortcut: 'h',
  when: hasListContext,
  run: () => void openListPicker('snooze'),
});
registerCommand({
  id: 'list-label',
  title: 'Label',
  section: 'Actions',
  shortcut: 'l',
  when: hasListContext,
  run: () => void openListPicker('label'),
});
registerCommand({
  id: 'list-move',
  title: 'Move to…',
  section: 'Actions',
  shortcut: 'v',
  when: hasListContext,
  run: () => void openListPicker('move'),
});
registerCommand({
  id: 'list-clear-selection',
  title: 'Clear selection',
  section: 'Actions',
  shortcut: 'esc',
  when: () => useSelection.getState().selectedIds.size > 0,
  run: () => useSelection.getState().clearSelection(),
});
// Not junk / Move to Inbox only exist where they can apply (P8.5), so the
// palette never offers an action that would do nothing.
registerCommand({
  id: 'list-not-junk',
  title: 'Not junk',
  section: 'Actions',
  when: () => useView.getState().view.kind === 'spam' && hasListContext(),
  run: () => void runMailCommand('notJunk', 'list'),
});
registerCommand({
  id: 'list-untrash',
  title: 'Move to Inbox',
  section: 'Actions',
  when: () => useView.getState().view.kind === 'trash' && hasListContext(),
  run: () => void runMailCommand('untrash', 'list'),
});
// Message-level copy actions (P8.5). They read the resolved target's own row,
// so a mixed selection copies the row the command named rather than guessing.
registerCommand({
  id: 'list-copy-address',
  title: 'Copy address',
  section: 'Actions',
  when: () => primaryRow(resolveCommandTargets('list')) !== undefined,
  run: () => {
    const row = primaryRow(resolveCommandTargets('list'));
    const address = row?.participants[0]?.e;
    if (!address) return;
    void copyToClipboard(address, 'Address');
  },
});
registerCommand({
  id: 'list-copy-subject',
  title: 'Copy subject',
  section: 'Actions',
  when: () => primaryRow(resolveCommandTargets('list')) !== undefined,
  run: () => {
    const row = primaryRow(resolveCommandTargets('list'));
    if (!row?.subject) return;
    void copyToClipboard(row.subject, 'Subject');
  },
});
