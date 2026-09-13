// View state -> components mapping (no router lib; view is state).
export const viewTitle: Record<string, string> = {
  inbox: 'Inbox',
  starred: 'Starred',
  snoozed: 'Snoozed',
  sent: 'Sent',
  drafts: 'Drafts',
  archive: 'Archive',
  // All Mail is a mailbox, not the unified account scope (P3.6): it lists every
  // conversation with a message outside Trash/Junk in the accounts in scope.
  all_mail: 'All Mail',
  spam: 'Spam',
  trash: 'Trash',
};
