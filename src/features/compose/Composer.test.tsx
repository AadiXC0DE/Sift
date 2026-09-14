import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, within } from '@testing-library/react';
import React from 'react';
import type { AttachmentRef } from '../../app/ipc/types';

const mocks = vi.hoisted(() => {
  const toast = Object.assign(vi.fn(), {
    error: vi.fn(),
    success: vi.fn(),
  });
  return {
    toast,
    openDialog: vi.fn(),
    api: {
      thread_get: vi.fn(),
      drafts_get: vi.fn(),
      drafts_upsert: vi.fn(),
      drafts_send: vi.fn(),
      drafts_delete: vi.fn(),
      send_cancel: vi.fn(),
      outbox_get: vi.fn(),
      outbox_retry: vi.fn(),
      contacts_suggest: vi.fn(),
      attachments_add_from_paths: vi.fn(),
      attachments_stage_from_message: vi.fn(),
      message_body: vi.fn(),
    },
  };
});

vi.mock('../../app/ipc/commands', () => ({ api: mocks.api }));
vi.mock('sonner', () => ({ toast: mocks.toast }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: mocks.openDialog }));
vi.mock('../../stores/accountsStore', () => ({
  useAccounts: (sel: (s: unknown) => unknown) =>
    sel({
      accounts: [
        { id: 'a1', email: 'me@x.com', signature_html: '<p>Sig One</p>' },
        { id: 'a2', email: 'alt@x.com', signature_html: '<p>Sig Two</p>' },
      ],
      included: { a1: true, a2: true },
      loading: false,
    }),
}));
vi.mock('../../stores/viewStore', () => ({
  useView: (sel: (s: unknown) => unknown) => sel({ accountScope: 'all' }),
}));
vi.mock('../../stores/settingsStore', () => ({
  useSettings: (sel: (s: unknown) => unknown) => sel({ settings: { undoSendDelay: 5 } }),
}));

import { ComposerSheet } from './ComposerSheet';
import type { Draft } from '../../app/ipc/types';

/** Every draft the composer persisted, in call order. */
function savedDrafts(): Draft[] {
  return mocks.api.drafts_upsert.mock.calls.map((call) => (call[0] as { draft: Draft }).draft);
}

function lastSavedDraft(): Draft {
  const all = savedDrafts();
  const last = all.at(-1);
  if (!last) throw new Error('drafts_upsert was never called');
  return last;
}

/** The arguments storage was handed for the last save. */
function lastSaveCall(): { draft: Draft; expectedRevision?: number } {
  const last = mocks.api.drafts_upsert.mock.calls.at(-1)?.[0] as
    { draft: Draft; expectedRevision?: number } | undefined;
  if (!last) throw new Error('drafts_upsert was never called');
  return last;
}

const ATT: AttachmentRef = {
  name: 'Report.pdf',
  mime: 'application/pdf',
  size: 2048,
  path: '/cache/staged/Report.pdf',
};

function renderComposer(onClose = vi.fn(), props: Record<string, unknown> = {}) {
  render(<ComposerSheet mode="new" onClose={onClose} {...props} />);
  return onClose;
}

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
  mocks.api.drafts_upsert.mockImplementation(async (args: unknown) => {
    const { draft } = args as { draft: Draft };
    return { ...draft, state: 'editing' as const, updatedAt: 1 };
  });
  mocks.api.drafts_get.mockRejectedValue(new Error('not found'));
  mocks.api.drafts_send.mockResolvedValue({ opId: 1, notBefore: 0 });
  mocks.api.drafts_delete.mockResolvedValue(undefined);
  mocks.api.send_cancel.mockResolvedValue(undefined);
  mocks.api.outbox_get.mockResolvedValue({
    opId: 1,
    accountId: 'a1',
    kind: 'send',
    state: 'pending',
    action: 'Send',
    recipientSummary: 'ben@y.org',
    subject: '',
    scheduledAt: null,
    retryAt: null,
    createdAt: 1,
    errorCode: null,
    errorMessage: null,
    draftId: null,
    revision: 1,
    requiresDuplicateAck: false,
  });
  mocks.api.outbox_retry.mockResolvedValue({ state: 'pending' });
  mocks.api.attachments_add_from_paths.mockResolvedValue([]);
  mocks.api.attachments_stage_from_message.mockResolvedValue(ATT);
  mocks.api.contacts_suggest.mockResolvedValue([]);
  mocks.api.message_body.mockResolvedValue({
    messageId: 'm1',
    state: 'ready',
    html: '<p>Full original body</p>',
    text: 'Full original body',
    remoteImageCount: 0,
    trackerCount: 0,
    darkSafe: true,
    remoteImagesAllowed: false,
  });
});

describe('P2.5 outgoing attachments', () => {
  it('adds the picker result as chips and persists the refs', async () => {
    mocks.openDialog.mockResolvedValue(['/Users/me/Report.pdf']);
    mocks.api.attachments_add_from_paths.mockResolvedValue([ATT]);
    renderComposer();

    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));

    await waitFor(() => expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy());
    expect(mocks.api.attachments_add_from_paths).toHaveBeenCalledWith({
      accountId: 'a1',
      draftId: expect.any(String),
      paths: ['/Users/me/Report.pdf'],
    });
    await waitFor(() => expect(mocks.api.drafts_upsert).toHaveBeenCalled());
    expect(lastSavedDraft().attachmentsJson).toEqual([ATT]);
  });

  it('a picker cancel changes nothing', async () => {
    mocks.openDialog.mockResolvedValue(null);
    renderComposer();

    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));

    await waitFor(() => expect(mocks.openDialog).toHaveBeenCalled());
    expect(mocks.api.attachments_add_from_paths).not.toHaveBeenCalled();
    expect(mocks.toast.error).not.toHaveBeenCalled();
  });

  it('shows per-file pending state and keeps the button disabled while staging', async () => {
    let release: (refs: AttachmentRef[]) => void = () => {};
    mocks.openDialog.mockResolvedValue(['/Users/me/Report.pdf']);
    mocks.api.attachments_add_from_paths.mockImplementation(
      () =>
        new Promise<AttachmentRef[]>((resolve) => {
          release = resolve;
        }),
    );
    renderComposer();

    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));

    await waitFor(() => expect(screen.getByText('Report.pdf')).toBeTruthy());
    expect(screen.getByTitle('Attach (⌘⇧A)')).toBeDisabled();

    release([ATT]);
    await waitFor(() => expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy());
    expect(screen.getByTitle('Attach (⌘⇧A)')).not.toBeDisabled();
  });

  it('surfaces the rejected filename and reason and keeps existing attachments', async () => {
    renderComposer();

    // First add one that succeeds so we can prove it survives the failure.
    mocks.openDialog.mockResolvedValue(['/Users/me/Report.pdf']);
    mocks.api.attachments_add_from_paths.mockResolvedValueOnce([ATT]);
    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));
    await waitFor(() => expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy());

    mocks.openDialog.mockResolvedValue(['/Users/me/Big.zip']);
    mocks.api.attachments_add_from_paths.mockRejectedValue({
      code: 'too_large',
      message: 'Attachments exceed 25 MB',
      retryable: false,
    });
    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));

    await waitFor(() => expect(mocks.toast.error).toHaveBeenCalled());
    const message = mocks.toast.error.mock.calls.at(-1)?.[0] as string;
    expect(message).toContain('Big.zip');
    expect(message).toContain('Attachments exceed 25 MB');
    expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy();
  });

  it('keeps both copies when the same filename is added twice', async () => {
    mocks.openDialog.mockResolvedValue(['/Users/me/Report.pdf']);
    mocks.api.attachments_add_from_paths
      .mockResolvedValueOnce([ATT])
      .mockResolvedValueOnce([{ ...ATT, path: '/cache/staged/second-Report.pdf' }]);
    renderComposer();

    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));
    await waitFor(() => expect(screen.getAllByLabelText('Remove Report.pdf')).toHaveLength(1));
    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));
    await waitFor(() => expect(screen.getAllByLabelText('Remove Report.pdf')).toHaveLength(2));

    await waitFor(() =>
      expect(lastSavedDraft().attachmentsJson.map((a) => a.path)).toEqual([
        '/cache/staged/Report.pdf',
        '/cache/staged/second-Report.pdf',
      ]),
    );
  });

  it('removes a single chip and re-persists the remaining refs', async () => {
    mocks.openDialog.mockResolvedValue(['/Users/me/Report.pdf']);
    mocks.api.attachments_add_from_paths.mockResolvedValueOnce([ATT]);
    renderComposer();
    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));
    await waitFor(() => expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Remove Report.pdf'));

    await waitFor(() => expect(screen.queryByLabelText('Remove Report.pdf')).toBeNull());
    await waitFor(() => expect(lastSavedDraft().attachmentsJson).toEqual([]));
  });
});

describe('P5.5 recipients', () => {
  it('keeps a quoted display name containing a comma in one chip', async () => {
    renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: '"Doe, John" <john@x.com>, ben@y.org' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(screen.getByLabelText('Remove Doe, John')).toBeTruthy());
    expect(screen.getByLabelText('Remove ben@y.org')).toBeTruthy();
  });

  it('sends with an empty To when Cc has a recipient', async () => {
    renderComposer();
    fireEvent.click(screen.getByLabelText('Show Cc recipients'));
    const ccInput = screen.getByLabelText('Cc recipients');
    fireEvent.change(ccInput, { target: { value: 'cara@z.org' } });
    fireEvent.keyDown(ccInput, { key: 'Enter' });

    fireEvent.click(screen.getByText('Send ⌘↵'));

    await waitFor(() => expect(mocks.api.drafts_send).toHaveBeenCalled());
    expect(mocks.api.drafts_send).toHaveBeenCalledWith(
      expect.objectContaining({
        localId: expect.any(String),
        revision: expect.any(Number),
        notBefore: expect.any(Number),
        archiveAfterSend: false,
      }),
    );
    expect(mocks.toast.error).not.toHaveBeenCalled();
  });

  it('refuses to send with no recipients at all', async () => {
    renderComposer();
    fireEvent.click(screen.getByText('Send ⌘↵'));

    await waitFor(() => expect(mocks.toast.error).toHaveBeenCalledWith('Add a recipient'));
    expect(mocks.api.drafts_send).not.toHaveBeenCalled();
  });

  it('commits half-typed address text on keyboard send', async () => {
    // Track which persisted draft each local ID came from, so the assertion
    // covers "what was sent", not just "something was saved at some point".
    const persisted: { localId: string; draft: Draft }[] = [];
    mocks.api.drafts_upsert.mockImplementation(async (args: unknown) => {
      const { draft } = args as { draft: Draft };
      persisted.push({ localId: draft.localId, draft });
      return { ...draft, state: 'editing' as const, updatedAt: 1 };
    });

    renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'ada@x.com' } });
    fireEvent.keyDown(window, { key: 'Enter', metaKey: true });

    // The half-typed address becomes a chip and is what the send persists.
    await waitFor(() => expect(screen.getByLabelText('Remove ada@x.com')).toBeTruthy());
    await waitFor(() => expect(mocks.api.drafts_send).toHaveBeenCalled());
    const sentLocalId = (mocks.api.drafts_send.mock.calls.at(-1)?.[0] as { localId: string }).localId;
    const sent = persisted.find((p) => p.localId === sentLocalId);
    expect(sent?.draft.toJson.map((a) => a.e)).toEqual(['ada@x.com']);
  });
});

describe('P5.5 suggestions', () => {
  it('shows suggestions as a listbox and dismisses them with Escape before the sheet', async () => {
    mocks.api.contacts_suggest.mockResolvedValue([
      { email: 'ada@x.com', name: 'Ada', lastUsedAt: 1, useCount: 3 },
    ]);
    const onClose = renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'ad' } });

    await waitFor(() => expect(screen.getByRole('listbox')).toBeTruthy());
    expect(mocks.api.contacts_suggest).toHaveBeenCalledWith('a1', 'ad', 6);
    const option = within(screen.getByRole('listbox')).getByRole('option');
    expect(option).toHaveTextContent('Ada <ada@x.com>');
    expect(input.getAttribute('aria-expanded')).toBe('true');
    await waitFor(() => expect(document.querySelector('[data-compose-escape="1"]')).toBeTruthy());

    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
    expect(onClose).not.toHaveBeenCalled();
    expect(document.querySelector('[data-compose-escape="1"]')).toBeNull();

    fireEvent.keyDown(input, { key: 'Escape' });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it('selects a suggestion with the keyboard', async () => {
    mocks.api.contacts_suggest.mockResolvedValue([
      { email: 'ada@x.com', name: 'Ada', lastUsedAt: 1, useCount: 3 },
      { email: 'adam@x.com', name: 'Adam', lastUsedAt: 1, useCount: 1 },
    ]);
    renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'ad' } });
    await waitFor(() => expect(screen.getByRole('listbox')).toBeTruthy());

    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(screen.getByLabelText('Remove Adam')).toBeTruthy());
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('offline suggestions fail quietly', async () => {
    mocks.api.contacts_suggest.mockRejectedValue(new Error('offline'));
    renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'ad' } });

    await waitFor(() => expect(mocks.api.contacts_suggest).toHaveBeenCalled());
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(screen.getByLabelText('To recipients')).toBeTruthy();
  });
});

describe('P5.5 reply and sender scope', () => {
  const detail = {
    accountId: 'a2',
    id: 't1',
    subject: 'Quarterly report',
    labelIds: [],
    messages: [
      {
        id: 'm1',
        from: { e: 'boss@x.com', n: 'Boss' },
        to: [{ e: 'me@x.com' }],
        cc: [],
        subject: 'Quarterly report',
        snippet: 'Please review',
      },
    ],
  };

  it('replying keeps the source account and its signature and suggests from it', async () => {
    mocks.api.thread_get.mockResolvedValue(detail);
    render(<ComposerSheet mode="reply" thread={{ accountId: 'a2', threadId: 't1' }} onClose={vi.fn()} />);

    await waitFor(() => expect(screen.getByLabelText('Remove Boss')).toBeTruthy());
    expect((screen.getByLabelText('From account') as HTMLSelectElement).value).toBe('a2');
    await waitFor(() => expect(document.querySelector('.tiptap')?.innerHTML ?? '').toContain('Sig Two'));
    expect(document.querySelector('.tiptap')?.innerHTML ?? '').not.toContain('Sig One');

    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'bo' } });
    await waitFor(() => expect(mocks.api.contacts_suggest).toHaveBeenCalledWith('a2', 'bo', 6));
  });

  it('changing From swaps only the signature block', async () => {
    renderComposer();
    await waitFor(() => expect(document.querySelector('.tiptap')?.innerHTML ?? '').toContain('Sig One'));

    fireEvent.change(screen.getByLabelText('From account'), { target: { value: 'a2' } });

    await waitFor(() => expect(document.querySelector('.tiptap')?.innerHTML ?? '').toContain('Sig Two'));
    const html = document.querySelector('.tiptap')?.innerHTML ?? '';
    expect(html).not.toContain('Sig One');
    expect(html.match(/data-sift-signature/g)).toHaveLength(1);
  });

  it('flushes a queued autosave when the sheet goes away', async () => {
    vi.useFakeTimers();
    try {
      const { unmount } = render(<ComposerSheet mode="new" onClose={vi.fn()} />);
      await vi.advanceTimersByTimeAsync(0);
      fireEvent.change(screen.getByLabelText('Subject'), { target: { value: 'Hello' } });
      // The signature insert queued the 300ms debounce; it has not fired yet.
      expect(document.querySelector('.tiptap')?.innerHTML ?? '').toContain('Sig One');
      expect(mocks.api.drafts_upsert).not.toHaveBeenCalled();

      unmount();
      await vi.advanceTimersByTimeAsync(0);

      // Exactly one save, carrying the state the user last saw.
      expect(mocks.api.drafts_upsert).toHaveBeenCalledTimes(1);
      const flushed = lastSavedDraft();
      expect(flushed.subject).toBe('Hello');
      expect(flushed.bodyHtml).toContain('Sig One');
    } finally {
      vi.useRealTimers();
    }
  });

  it('unified scope restores the last deliberately used sender', async () => {
    window.localStorage.setItem('sift.compose.lastSender', 'a2');
    renderComposer();

    await waitFor(() =>
      expect((screen.getByLabelText('From account') as HTMLSelectElement).value).toBe('a2'),
    );
    await waitFor(() => expect(document.querySelector('.tiptap')?.innerHTML ?? '').toContain('Sig Two'));
  });
});

describe('P5.5 signature and formatting', () => {
  it('applies the sending account signature once on a new message', async () => {
    renderComposer();
    await waitFor(() => {
      const html = document.querySelector('.tiptap')?.innerHTML ?? '';
      expect(html).toContain('Sig One');
    });
    const html = document.querySelector('.tiptap')?.innerHTML ?? '';
    expect(html.match(/data-sift-signature/g)).toHaveLength(1);
  });

  it('exposes bold/italic/list/quote/link and a plain-text toggle', () => {
    renderComposer();
    expect(screen.getByRole('toolbar', { name: 'Formatting' })).toBeTruthy();
    for (const name of ['Bold', 'Italic', 'Bulleted list', 'Numbered list', 'Quote', 'Link']) {
      expect(screen.getByRole('button', { name })).toBeTruthy();
    }
    const plain = screen.getByRole('button', { name: 'Plain text' });
    expect(plain.getAttribute('aria-pressed')).toBe('false');
    fireEvent.click(plain);
    expect(screen.getByRole('button', { name: 'Bold' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Plain text' }).getAttribute('aria-pressed')).toBe('true');
  });
});

describe('P5.1 one identity and every-edit persistence', () => {
  it('allocates one draft id at open and reuses it for every later save', async () => {
    renderComposer();
    const subjectInput = screen.getByLabelText('Subject');
    fireEvent.change(subjectInput, { target: { value: 'First' } });
    await waitFor(() => expect(mocks.api.drafts_upsert).toHaveBeenCalled());
    fireEvent.change(subjectInput, { target: { value: 'Second' } });
    await waitFor(() => expect(savedDrafts().length).toBeGreaterThan(1));

    const ids = savedDrafts().map((d) => d.localId);
    expect(new Set(ids).size).toBe(1);
    const revisions = savedDrafts().map((d) => d.revision);
    expect(revisions).toEqual([...revisions].sort((a, b) => a - b));
    expect(revisions.at(-1)).toBeGreaterThan(revisions[0]);
    expect(lastSavedDraft().subject).toBe('Second');
  });

  it('a subject-only edit followed by an immediate close persists exactly one draft', async () => {
    const onClose = renderComposer();
    await waitFor(() => expect(document.querySelector('.tiptap')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Subject'), { target: { value: 'Only a subject' } });
    fireEvent.click(screen.getByRole('button', { name: 'Close composer' }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mocks.api.drafts_upsert).toHaveBeenCalledTimes(1);
    expect(lastSavedDraft().subject).toBe('Only a subject');
  });

  it('persists an attachment-only change', async () => {
    mocks.openDialog.mockResolvedValue(['/Users/me/Report.pdf']);
    mocks.api.attachments_add_from_paths.mockResolvedValue([ATT]);
    renderComposer();
    await waitFor(() => expect(document.querySelector('.tiptap')).toBeTruthy());

    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));

    await waitFor(() => expect(lastSavedDraft().attachmentsJson).toEqual([ATT]));
    expect(mocks.api.drafts_upsert).toHaveBeenCalledTimes(1);
  });

  it('persists a From switch with the new account and the same id', async () => {
    renderComposer();
    fireEvent.change(screen.getByLabelText('From account'), { target: { value: 'a2' } });

    await waitFor(() => expect(lastSavedDraft().accountId).toBe('a2'));
    expect(new Set(savedDrafts().map((d) => d.localId)).size).toBe(1);
    expect(lastSavedDraft().fromEmail).toBe('alt@x.com');
  });

  it('keeps the composer open and names storage when the save fails, then retries', async () => {
    const onClose = vi.fn();
    mocks.api.drafts_upsert.mockRejectedValueOnce(new Error('quota'));
    renderComposer(onClose);
    await waitFor(() => expect(document.querySelector('.tiptap')).toBeTruthy());

    fireEvent.change(screen.getByLabelText('Subject'), { target: { value: 'At risk' } });
    fireEvent.click(screen.getByRole('button', { name: 'Close composer' }));

    // A failed save is a storage failure with a retry, never an offline claim.
    const status = await screen.findByText('Couldn’t save');
    expect(status.textContent).not.toMatch(/offline/i);
    expect(status.getAttribute('title')).toMatch(/storage/i);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Subject')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(screen.getByText('Saved')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'Close composer' }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });
});

describe('P5.2 reopening a stored draft', () => {
  const stored: Draft = {
    localId: 'draft-7',
    accountId: 'a2',
    fromEmail: 'alt@x.com',
    mode: 'reply',
    threadId: 't1',
    inReplyToMessageId: 'm1',
    rfcMessageId: '<m1@x.com>',
    toJson: [{ e: 'boss@x.com', n: 'Boss' }],
    ccJson: [{ e: 'cara@z.org' }],
    bccJson: [],
    subject: 'Re: Quarterly report',
    bodyHtml: '<p>my drafted reply</p>',
    attachmentsJson: [ATT],
    revision: 7,
    state: 'editing',
    updatedAt: 5,
  };

  it('restores every field and attachment and keeps the stored id and revision', async () => {
    mocks.api.drafts_get.mockResolvedValue(stored);
    render(<ComposerSheet mode="reply" draftId="draft-7" onClose={vi.fn()} />);

    await waitFor(() => expect(screen.getByLabelText('Remove Boss')).toBeTruthy());
    expect((screen.getByLabelText('Subject') as HTMLInputElement).value).toBe('Re: Quarterly report');
    expect(screen.getByLabelText('Remove cara@z.org')).toBeTruthy();
    expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy();
    expect((screen.getByLabelText('From account') as HTMLSelectElement).value).toBe('a2');
    await waitFor(() =>
      expect(document.querySelector('.tiptap')?.innerHTML ?? '').toContain('my drafted reply'),
    );
    // The stored draft produced no write of its own.
    expect(mocks.api.drafts_upsert).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText('Subject'), { target: { value: 'Re: edited' } });

    await waitFor(() => expect(mocks.api.drafts_upsert).toHaveBeenCalled());
    const call = lastSaveCall();
    expect(call.draft.localId).toBe('draft-7');
    expect(call.draft.revision).toBeGreaterThan(7);
    expect(call.expectedRevision).toBe(7);
    expect(call.draft.inReplyToMessageId).toBe('m1');
    expect(call.draft.rfcMessageId).toBe('<m1@x.com>');
  });

  it('reports a draft that storage no longer has instead of opening a new one', async () => {
    mocks.api.drafts_get.mockRejectedValue(new Error('draft_not_found'));
    render(<ComposerSheet mode="reply" draftId="gone" onClose={vi.fn()} />);

    await waitFor(() => expect(mocks.toast.error).toHaveBeenCalledWith('Could not open this draft'));
    expect(mocks.api.drafts_upsert).not.toHaveBeenCalled();
  });
});

describe('P5.4 reply, reply-all and forward', () => {
  const base = {
    accountId: 'a2',
    id: 't1',
    subject: 'Quarterly report',
    labelIds: [],
    messages: [
      {
        id: 'm1',
        internalDate: 1_760_000_000_000,
        from: { e: 'boss@x.com', n: 'Boss' },
        to: [{ e: 'me@x.com' }],
        bcc: [],
        replyTo: '',
        subject: 'Quarterly report',
        snippet: 'Please review',
        isDraft: false,
        isSentByMe: false,
        labelIds: [],
        hasAttachments: false,
        attachments: [],
        bodyState: 'fetched',
      },
    ],
  };

  it('uses Reply-To instead of From and quotes the full body with attribution', async () => {
    mocks.api.thread_get.mockResolvedValue({
      ...base,
      messages: [{ ...base.messages[0], replyTo: 'Support <support@example.test>' }],
    });
    mocks.api.message_body.mockResolvedValue({
      messageId: 'm1',
      state: 'ready',
      html: `<p>${'the whole readable message. '.repeat(20)}TRAILING FACT</p>`,
      remoteImageCount: 0,
      trackerCount: 0,
      darkSafe: true,
      remoteImagesAllowed: false,
    });
    render(<ComposerSheet mode="reply" thread={{ accountId: 'a2', threadId: 't1' }} onClose={vi.fn()} />);

    await waitFor(() => expect(screen.getByLabelText('Remove Support')).toBeTruthy());
    expect(screen.queryByLabelText('Remove Boss')).toBeNull();
    const html = document.querySelector('.tiptap')?.innerHTML ?? '';
    expect(html).toContain('TRAILING FACT');
    // The quote is attributed to the original sender, not to the reply target.
    expect(html).toContain('Boss &lt;boss@x.com&gt;');
    expect(html).toContain('data-sift-quote');
    expect(html).toContain('<details');
    expect((screen.getByLabelText('Subject') as HTMLInputElement).value).toBe('Re: Quarterly report');

    await waitFor(() => expect(mocks.api.drafts_upsert).toHaveBeenCalled());
    const draft = lastSavedDraft();
    expect(draft.inReplyToMessageId).toBe('m1');
    expect(draft.subject).toBe('Re: Quarterly report');
  });

  it('reply-all drops own identities and never exposes a prior Bcc', async () => {
    mocks.api.thread_get.mockResolvedValue({
      ...base,
      messages: [
        {
          ...base.messages[0],
          from: { e: 'boss@x.com', n: 'Boss' },
          to: [{ e: 'me@x.com' }, { e: 'ada@x.com', n: 'Ada' }],
          cc: [{ e: 'cara@z.org' }, { e: 'Boss@X.com' }],
          bcc: [{ e: 'secret@x.com' }],
        },
      ],
    });
    render(<ComposerSheet mode="reply_all" thread={{ accountId: 'a2', threadId: 't1' }} onClose={vi.fn()} />);

    await waitFor(() => expect(screen.getByLabelText('Remove Boss')).toBeTruthy());
    expect(screen.getByLabelText('Remove Ada')).toBeTruthy();
    expect(screen.getByLabelText('Remove cara@z.org')).toBeTruthy();
    const chips = screen
      .getAllByRole('button', { name: /^Remove / })
      .map((el) => el.getAttribute('aria-label'));
    expect(chips).not.toContain('Remove me@x.com');
    expect(chips).not.toContain('Remove secret@x.com');
    // The duplicate of the reply target lives in exactly one field.
    expect(chips.filter((c) => c === 'Remove Boss')).toHaveLength(1);
  });

  it('replying to your own sent message targets the original recipients', async () => {
    mocks.api.thread_get.mockResolvedValue({
      ...base,
      messages: [
        {
          ...base.messages[0],
          from: { e: 'alt@x.com' },
          to: [{ e: 'ada@x.com' }],
          cc: [],
          isSentByMe: true,
        },
      ],
    });
    render(<ComposerSheet mode="reply" thread={{ accountId: 'a2', threadId: 't1' }} onClose={vi.fn()} />);

    await waitFor(() => expect(screen.getByLabelText('Remove ada@x.com')).toBeTruthy());
    expect(screen.queryByLabelText('Remove alt@x.com')).toBeNull();
  });

  it('replies to the explicitly selected message rather than the newest one', async () => {
    mocks.api.thread_get.mockResolvedValue({
      ...base,
      messages: [
        { ...base.messages[0], id: 'm1', from: { e: 'first@x.com' } },
        { ...base.messages[0], id: 'm2', from: { e: 'newest@x.com' } },
      ],
    });
    render(
      <ComposerSheet
        mode="reply"
        thread={{ accountId: 'a2', threadId: 't1', messageId: 'm1' }}
        onClose={vi.fn()}
      />,
    );

    await waitFor(() => expect(screen.getByLabelText('Remove first@x.com')).toBeTruthy());
    expect(screen.queryByLabelText('Remove newest@x.com')).toBeNull();
  });

  it('keeps the body inert until the context is ready and initializes the quote once', async () => {
    let releaseBody: (b: unknown) => void = () => {};
    mocks.api.thread_get.mockResolvedValue(base);
    mocks.api.message_body.mockImplementation(
      () =>
        new Promise((resolve) => {
          releaseBody = resolve;
        }),
    );
    render(<ComposerSheet mode="reply" thread={{ accountId: 'a2', threadId: 't1' }} onClose={vi.fn()} />);

    await waitFor(() => expect(screen.getByText(/Loading the message being answered/)).toBeTruthy());
    const editorEl = document.querySelector('.tiptap');
    expect(editorEl?.getAttribute('contenteditable')).toBe('false');

    releaseBody({
      messageId: 'm1',
      state: 'ready',
      html: '<p>Original body</p>',
      remoteImageCount: 0,
      trackerCount: 0,
      darkSafe: true,
      remoteImagesAllowed: false,
    });

    await waitFor(() =>
      expect(document.querySelector('.tiptap')?.getAttribute('contenteditable')).toBe('true'),
    );
    const html = document.querySelector('.tiptap')?.innerHTML ?? '';
    expect(html).toContain('Original body');
    // The quote lives in the document, so a collapsed drawer still sends it.
    expect(html).toContain('data-sift-quote');
    expect(html).toContain('blockquote');
  });

  it('says the original could not be loaded instead of failing the composer', async () => {
    mocks.api.thread_get.mockRejectedValue(new Error('offline'));
    render(<ComposerSheet mode="reply" thread={{ accountId: 'a2', threadId: 't1' }} onClose={vi.fn()} />);

    await waitFor(() => expect(screen.getByText(/could not be loaded/)).toBeTruthy());
    expect(screen.getByLabelText('Subject')).toBeTruthy();
  });

  it('forward includes the full body, carries no reply threading and offers original attachments', async () => {
    mocks.api.thread_get.mockResolvedValue({
      ...base,
      messages: [
        {
          ...base.messages[0],
          rfcMessageId: '<m1@x.com>',
          hasAttachments: true,
          attachments: [
            {
              id: 'att1',
              filename: 'Plan.pdf',
              mime: 'application/pdf',
              size: 4096,
              isInline: false,
              downloaded: false,
            },
            {
              id: 'att2',
              filename: 'logo.png',
              mime: 'image/png',
              size: 100,
              isInline: true,
              downloaded: true,
            },
          ],
        },
      ],
    });
    mocks.api.message_body.mockResolvedValue({
      messageId: 'm1',
      state: 'ready',
      text: 'forwarded full body',
      remoteImageCount: 0,
      trackerCount: 0,
      darkSafe: true,
      remoteImagesAllowed: false,
    });
    render(<ComposerSheet mode="forward" thread={{ accountId: 'a2', threadId: 't1' }} onClose={vi.fn()} />);

    await waitFor(() => expect(screen.getByText('forwarded full body')).toBeTruthy());
    expect((screen.getByLabelText('Subject') as HTMLInputElement).value).toBe('Fwd: Quarterly report');
    // Inline images are not offered as forwardable attachments.
    expect(screen.getByLabelText('Include Plan.pdf')).toBeTruthy();
    expect(screen.queryByLabelText('Include logo.png')).toBeNull();

    fireEvent.click(screen.getByLabelText('Include Plan.pdf'));

    await waitFor(() =>
      expect(mocks.api.attachments_stage_from_message).toHaveBeenCalledWith({
        accountId: 'a2',
        messageId: 'm1',
        attachmentId: 'att1',
        draftId: expect.any(String),
      }),
    );
    await waitFor(() => expect(lastSavedDraft().attachmentsJson.map((a) => a.name)).toEqual(['Report.pdf']));

    const draft = lastSavedDraft();
    expect(draft.inReplyToMessageId).toBeUndefined();
    expect(draft.rfcMessageId).toBeUndefined();
    expect(draft.threadId).toBe('t1');
  });

  it('shows a failed original-attachment copy instead of pretending it worked', async () => {
    mocks.api.thread_get.mockResolvedValue({
      ...base,
      messages: [
        {
          ...base.messages[0],
          attachments: [
            {
              id: 'att1',
              filename: 'Plan.pdf',
              mime: 'application/pdf',
              size: 4096,
              isInline: false,
              downloaded: false,
            },
          ],
        },
      ],
    });
    mocks.api.attachments_stage_from_message.mockRejectedValue(new Error('attachment_failed'));
    render(<ComposerSheet mode="forward" thread={{ accountId: 'a2', threadId: 't1' }} onClose={vi.fn()} />);

    fireEvent.click(await screen.findByLabelText('Include Plan.pdf'));

    await waitFor(() => expect(screen.getByText('Couldn’t copy')).toBeTruthy());
    expect(mocks.toast.error).toHaveBeenCalledWith(expect.stringContaining('Plan.pdf'));
    expect(mocks.api.attachments_stage_from_message).toHaveBeenCalled();
  });
});

afterEach(cleanup);

/**
 * P6.2 (SEND-04): queueing is not sending. The composer says "Queued · Undo"
 * until the provider accepts, and Undo reopens the draft the backend returned.
 */
describe('P6.2 honest queued send and atomic undo', () => {
  async function sendOnce() {
    renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'ben@y.org' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    fireEvent.change(screen.getByLabelText('Subject'), { target: { value: 'Queued mail' } });
    fireEvent.click(screen.getByText('Send ⌘↵'));
    await waitFor(() => expect(mocks.api.drafts_send).toHaveBeenCalled());
  }

  it('says Queued · Undo, never Sent, while the operation is pending', async () => {
    await sendOnce();

    const banner = await screen.findByTestId('compose-queued');
    expect(banner).toHaveTextContent('Queued · Undo');
    expect(banner).not.toHaveTextContent(/\bSent\b/);
    expect(screen.queryByText('✓')).toBeNull();
  });

  it('Undo cancels the operation and refills the composer from the returned draft', async () => {
    await sendOnce();
    mocks.api.send_cancel.mockResolvedValue({
      localId: 'draft-1',
      accountId: 'a1',
      mode: 'new',
      toJson: [{ e: 'ben@y.org' }],
      ccJson: [],
      bccJson: [],
      subject: 'Queued mail',
      bodyHtml: '<p>restored body</p>',
      attachmentsJson: [ATT],
      revision: 3,
      state: 'editing',
    });

    fireEvent.click(await screen.findByText('Undo'));

    await waitFor(() => expect(mocks.api.send_cancel).toHaveBeenCalledWith({ opId: 1 }));
    expect(await screen.findByLabelText('Remove ben@y.org')).toBeTruthy();
    expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy();
    expect(screen.getByLabelText('Subject')).toHaveValue('Queued mail');
    expect(document.querySelector('.tiptap')?.innerHTML).toContain('restored body');
    // Back to editing: the queued banner and the frozen state are gone.
    expect(screen.queryByTestId('compose-queued')).toBeNull();
    expect(screen.getByText('Send ⌘↵')).toBeTruthy();

    // The restored revision is the one storage acknowledged: the next edit is a
    // new revision, not a write against a stale one.
    fireEvent.change(screen.getByLabelText('Subject'), { target: { value: 'Queued mail (edited)' } });
    await waitFor(() => expect(lastSaveCall().expectedRevision).toBe(3));
  });

  it('shows the honest state when the undo window has closed', async () => {
    await sendOnce();
    mocks.api.send_cancel.mockRejectedValue({
      code: 'send_undo_expired',
      message: 'This message was already sent',
      retryable: false,
      detail: { state: 'done', opId: 1, notBefore: 0 },
    });

    fireEvent.click(await screen.findByText('Undo'));

    // The state comes from the typed error, not from the undo attempt: the
    // composer reports what the provider actually did.
    const banner = await screen.findByTestId('compose-queued');
    await waitFor(() => expect(banner).toHaveTextContent('Sent'));
    expect(mocks.toast.error).toHaveBeenCalledWith('This message was already sent');
    // The draft was never refilled with a body it does not have.
    expect(document.querySelector('.tiptap')?.textContent ?? '').not.toContain('restored body');
  });

  it('a failed send says Not sent, keeps the composer open and offers Retry', async () => {
    mocks.api.outbox_get.mockResolvedValue({
      opId: 1,
      accountId: 'a1',
      kind: 'send',
      state: 'failed',
      action: 'Send',
      recipientSummary: 'ben@y.org',
      subject: 'Queued mail',
      scheduledAt: null,
      retryAt: null,
      createdAt: 1,
      errorCode: 'smtp_rejected',
      errorMessage: 'The provider rejected this message',
      draftId: null,
      revision: 1,
      requiresDuplicateAck: false,
    });
    const onClose = renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'ben@y.org' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    fireEvent.click(screen.getByText('Send ⌘↵'));

    const banner = await screen.findByTestId('compose-queued');
    await waitFor(() => expect(banner).toHaveTextContent('Not sent'));
    expect(banner).not.toHaveTextContent('Queued · Undo');
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText('Retry'));
    await waitFor(() =>
      expect(mocks.api.outbox_retry).toHaveBeenCalledWith({
        opId: 1,
        acknowledgeDuplicateRisk: false,
      }),
    );
  });

  it('send and archive queues the archive with the send instead of archiving now', async () => {
    renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'ben@y.org' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    fireEvent.click(screen.getByText('Send & archive'));

    await waitFor(() =>
      expect(mocks.api.drafts_send).toHaveBeenCalledWith(expect.objectContaining({ archiveAfterSend: true })),
    );
  });
});
