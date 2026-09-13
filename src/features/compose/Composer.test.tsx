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
      drafts_upsert: vi.fn(),
      drafts_send: vi.fn(),
      drafts_delete: vi.fn(),
      send_cancel: vi.fn(),
      contacts_suggest: vi.fn(),
      attachments_add_from_paths: vi.fn(),
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
  return mocks.api.drafts_upsert.mock.calls.map((call) => call[0] as Draft);
}

function lastSavedDraft(): Draft {
  const all = savedDrafts();
  const last = all.at(-1);
  if (!last) throw new Error('drafts_upsert was never called');
  return last;
}

const ATT: AttachmentRef = {
  name: 'Report.pdf',
  mime: 'application/pdf',
  size: 2048,
  path: '/cache/staged/Report.pdf',
};

function renderComposer(onClose = vi.fn()) {
  render(<ComposerSheet mode="new" onClose={onClose} />);
  return onClose;
}

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
  mocks.api.drafts_upsert.mockImplementation(async (d: unknown) => ({ ...(d as object), localId: 'd1' }));
  mocks.api.drafts_send.mockResolvedValue({ op_id: 1 });
  mocks.api.drafts_delete.mockResolvedValue(undefined);
  mocks.api.send_cancel.mockResolvedValue(undefined);
  mocks.api.attachments_add_from_paths.mockResolvedValue([]);
  mocks.api.contacts_suggest.mockResolvedValue([]);
});

describe('P2.5 outgoing attachments', () => {
  it('adds the picker result as chips and persists the refs', async () => {
    mocks.openDialog.mockResolvedValue(['/Users/me/Report.pdf']);
    mocks.api.attachments_add_from_paths.mockResolvedValue([ATT]);
    renderComposer();

    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));

    await waitFor(() => expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy());
    expect(mocks.api.attachments_add_from_paths).toHaveBeenCalledWith(['/Users/me/Report.pdf']);
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

    expect(lastSavedDraft().attachmentsJson.map((a) => a.path)).toEqual([
      '/cache/staged/Report.pdf',
      '/cache/staged/second-Report.pdf',
    ]);
  });

  it('removes a single chip and re-persists the remaining refs', async () => {
    mocks.openDialog.mockResolvedValue(['/Users/me/Report.pdf']);
    mocks.api.attachments_add_from_paths.mockResolvedValueOnce([ATT]);
    renderComposer();
    fireEvent.click(screen.getByTitle('Attach (⌘⇧A)'));
    await waitFor(() => expect(screen.getByLabelText('Remove Report.pdf')).toBeTruthy());

    fireEvent.click(screen.getByLabelText('Remove Report.pdf'));

    await waitFor(() => expect(screen.queryByLabelText('Remove Report.pdf')).toBeNull());
    expect(lastSavedDraft().attachmentsJson).toEqual([]);
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

    await waitFor(() => expect(mocks.api.drafts_send).toHaveBeenCalledWith('d1', 5000));
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
    mocks.api.drafts_upsert.mockImplementation(async (d: unknown) => {
      const localId = `d${persisted.length + 1}`;
      persisted.push({ localId, draft: d as Draft });
      return { ...(d as object), localId };
    });

    renderComposer();
    const input = screen.getByLabelText('To recipients');
    fireEvent.change(input, { target: { value: 'ada@x.com' } });
    fireEvent.keyDown(window, { key: 'Enter', metaKey: true });

    // The half-typed address becomes a chip and is what the send persists.
    await waitFor(() => expect(screen.getByLabelText('Remove ada@x.com')).toBeTruthy());
    await waitFor(() => expect(mocks.api.drafts_send).toHaveBeenCalled());
    const sentLocalId = mocks.api.drafts_send.mock.calls.at(-1)?.[0];
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
    expect(onClose).toHaveBeenCalledTimes(1);
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

afterEach(cleanup);
