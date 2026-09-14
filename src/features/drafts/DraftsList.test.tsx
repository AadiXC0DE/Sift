import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, act } from '@testing-library/react';
import React from 'react';
import type { AttachmentRef, Draft } from '../../app/ipc/types';

const mocks = vi.hoisted(() => ({
  draftsList: vi.fn(),
  accounts: [
    { id: 'a1', email: 'me@x.com' },
    { id: 'a2', email: 'alt@x.com' },
  ],
}));

vi.mock('../../app/ipc/commands', () => ({ api: { drafts_list: mocks.draftsList } }));
vi.mock('../../stores/accountsStore', () => ({
  useAccounts: (sel: (s: unknown) => unknown) => sel({ accounts: mocks.accounts }),
}));

type StoreHandler = (payload: unknown) => void;
const listeners = new Map<string, Set<StoreHandler>>();

vi.mock('../../app/ipc/events', () => ({
  on: (event: string, handler: StoreHandler) => {
    const set = listeners.get(event) ?? new Set<StoreHandler>();
    set.add(handler);
    listeners.set(event, set);
    return Promise.resolve(() => set.delete(handler));
  },
}));

function emit(event: string, payload: unknown) {
  for (const handler of listeners.get(event) ?? []) handler(payload);
}

import { DraftsList } from './DraftsList';

const ATTACHMENT: AttachmentRef = {
  name: 'report.pdf',
  mime: 'application/pdf',
  size: 1024,
  path: '/tmp/report.pdf',
};

/**
 * A draft carrying the full persisted contract. The cast is required because
 * `state`/`localId` land with the drafts_list command; the fixture already
 * matches the shape the list reads.
 */
function makeDraft(localId: string, over: Record<string, unknown> = {}): Draft {
  return {
    localId,
    accountId: 'a1',
    mode: 'new',
    toJson: [],
    ccJson: [],
    bccJson: [],
    subject: localId,
    bodyHtml: '',
    attachmentsJson: [],
    revision: 1,
    state: 'editing',
    ...over,
  } as Draft;
}

async function tick(times = 6) {
  for (let i = 0; i < times; i++) await act(async () => void (await Promise.resolve()));
}

function renderList(accountIds: string[] = ['a1'], onOpenDraft = vi.fn()) {
  render(<DraftsList accountIds={accountIds} onOpenDraft={onOpenDraft} />);
  return { onOpenDraft };
}

beforeEach(() => {
  mocks.draftsList.mockReset();
  listeners.clear();
});

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

describe('DraftsList rows', () => {
  it('renders account, recipients, subject, snippet, attachments and updated time in backend order', async () => {
    const withExtras = makeDraft('d1', {
      subject: 'Zebra report',
      bodyHtml: '<p>Hello    world</p>',
      toJson: [{ e: 'amy@x.com', n: 'Amy' }],
      ccJson: [{ e: 'bob@x.com' }],
      bccJson: [{ e: 'cara@x.com' }, { e: 'dan@x.com' }],
      attachmentsJson: [ATTACHMENT, { ...ATTACHMENT, name: 'notes.txt' }],
      updatedAt: Date.now() - 185_000,
    });
    const ccOnly = makeDraft('d2', {
      accountId: 'a2',
      subject: 'Alpha plan',
      ccJson: [{ e: 'bob@x.com', n: 'Bob' }],
    });
    mocks.draftsList.mockResolvedValue({ drafts: [withExtras, ccOnly] });

    renderList(['a1', 'a2']);

    const rows = await screen.findAllByTestId('draft-row');
    expect(rows).toHaveLength(2);
    // Backend order is the render order, even though "Alpha" sorts first.
    expect(rows[0]).toHaveAttribute('aria-label', 'Open draft: Zebra report');
    expect(rows[1]).toHaveAttribute('aria-label', 'Open draft: Alpha plan');

    expect(screen.getByText('me@x.com')).toBeInTheDocument();
    expect(screen.getByText('alt@x.com')).toBeInTheDocument();
    expect(screen.getByText(/^To: /)).toHaveTextContent('"Amy" <amy@x.com>');
    expect(screen.getByText('+1 more')).toBeInTheDocument();
    expect(screen.getByText(/^Cc: /)).toHaveTextContent('"Bob" <bob@x.com>');
    expect(screen.getByText('Zebra report')).toBeInTheDocument();
    expect(screen.getByText('Hello world')).toBeInTheDocument();
    expect(screen.getByLabelText('2 attachments')).toBeInTheDocument();
    expect(screen.getByText('3 min ago')).toBeInTheDocument();
  });

  it('falls back to (No subject) and labels only non-editing states', async () => {
    mocks.draftsList.mockResolvedValue({
      drafts: [
        makeDraft('d1', { subject: '   ', state: 'queued' }),
        makeDraft('d2', { subject: 'Later', state: 'sent' }),
        makeDraft('d3', { subject: 'Plain', state: 'editing' }),
      ],
    });

    renderList();

    const rows = await screen.findAllByTestId('draft-row');
    expect(rows[0]).toHaveAttribute('aria-label', 'Open draft: (No subject)');
    expect(screen.getByText('(No subject)')).toBeInTheDocument();
    expect(screen.getByText('Queued')).toBeInTheDocument();
    expect(screen.getByText('Sent')).toBeInTheDocument();
    expect(screen.queryByText('Editing')).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/attachments/)).not.toBeInTheDocument();
  });
});

describe('DraftsList paging', () => {
  it('loads more with the returned cursor and renders a draft that appears twice once', async () => {
    const first = makeDraft('d1', { subject: 'First' });
    const second = makeDraft('d2', { subject: 'Second' });
    mocks.draftsList
      .mockResolvedValueOnce({ drafts: [first], nextCursor: 'c1' })
      .mockResolvedValueOnce({ drafts: [first, second] });

    renderList();

    await screen.findByTestId('draft-row');
    fireEvent.click(screen.getByRole('button', { name: 'Load more' }));

    await waitFor(() => expect(screen.getAllByTestId('draft-row')).toHaveLength(2));
    expect(mocks.draftsList.mock.calls[1][0]).toEqual({
      accountIds: ['a1'],
      cursor: 'c1',
      limit: 30,
    });
    expect(screen.getByText('Second')).toBeInTheDocument();
  });

  it('offers no Load more button on the last page', async () => {
    mocks.draftsList.mockResolvedValue({ drafts: [makeDraft('d1')] });

    renderList();

    await screen.findByTestId('draft-row');
    expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
  });
});

describe('DraftsList opening', () => {
  it('hands the whole draft to onOpenDraft when a row is clicked', async () => {
    const draft = makeDraft('d1', { subject: 'Draft me', toJson: [{ e: 'amy@x.com' }] });
    mocks.draftsList.mockResolvedValue({ drafts: [draft] });
    const { onOpenDraft } = renderList();

    fireEvent.click(await screen.findByTestId('draft-row'));

    expect(onOpenDraft).toHaveBeenCalledTimes(1);
    expect(onOpenDraft.mock.calls[0][0]).toEqual(draft);
  });
});

describe('DraftsList failures', () => {
  it('shows the error and re-fetches from Retry', async () => {
    mocks.draftsList
      .mockRejectedValueOnce(new Error('Backend exploded'))
      .mockResolvedValue({ drafts: [makeDraft('d1', { subject: 'Recovered' })] });

    renderList();

    expect(await screen.findByText('Backend exploded')).toBeInTheDocument();
    expect(screen.queryAllByTestId('draft-row')).toHaveLength(0);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('Recovered')).toBeInTheDocument();
    expect(mocks.draftsList).toHaveBeenCalledTimes(2);
    expect(mocks.draftsList.mock.calls[1][0]).toEqual({
      accountIds: ['a1'],
      cursor: undefined,
      limit: 30,
    });
  });
});

describe('DraftsList store events', () => {
  it('refreshes so a draft saved by the composer appears without a manual reload', async () => {
    vi.useFakeTimers();
    mocks.draftsList
      .mockResolvedValueOnce({ drafts: [makeDraft('d1', { subject: 'Older draft' })] })
      .mockResolvedValue({
        drafts: [makeDraft('d2', { subject: 'Saved later' }), makeDraft('d1', { subject: 'Older draft' })],
      });

    renderList();
    await tick();
    expect(screen.queryByText('Saved later')).not.toBeInTheDocument();

    act(() => {
      emit('store:drafts', { account_id: 'a1', draft_id: 'd2' });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });

    expect(screen.getByText('Saved later')).toBeInTheDocument();
    expect(screen.getByText('Older draft')).toBeInTheDocument();
  });
});
