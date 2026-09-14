import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import React from 'react';
import type { OutboxOp } from '../../app/ipc/types';

const mocks = vi.hoisted(() => ({
  api: { outbox_list: vi.fn(), outbox_retry: vi.fn() },
}));

vi.mock('../../app/ipc/commands', () => ({ api: mocks.api }));
vi.mock('../../stores/accountsStore', () => ({
  useAccounts: (sel: (s: unknown) => unknown) =>
    sel({ accounts: [{ id: 'acc-a', email: 'me@x.com' }], included: { 'acc-a': true } }),
}));

import { OutboxPanel } from './OutboxPanel';
import { useOutbox, EMPTY_COUNTS, OUTBOX_PAGE_LIMIT } from '../../stores/outboxStore';
import type { OutboxPage } from '../../app/ipc/types';

const COUNTS = { pending: 0, inflight: 0, uncertain: 0, done: 0, failed: 0, cancelled: 0 };

function op(over: Partial<OutboxOp>): OutboxOp {
  return {
    opId: 1,
    accountId: 'acc-a',
    kind: 'send',
    state: 'pending',
    action: 'Send',
    recipientSummary: 'ben@y.org',
    subject: 'Quarterly report',
    scheduledAt: null,
    retryAt: null,
    createdAt: Date.now(),
    errorCode: null,
    errorMessage: null,
    draftId: 'd1',
    revision: 1,
    requiresDuplicateAck: false,
    ...over,
  };
}

function page(operations: OutboxOp[], over: Partial<OutboxPage> = {}): OutboxPage {
  return { operations, nextCursor: null, total: operations.length, counts: COUNTS, ...over };
}

beforeEach(() => {
  vi.clearAllMocks();
  useOutbox.setState({
    open: true,
    operations: [],
    counts: EMPTY_COUNTS,
    total: 0,
    nextCursor: null,
    loading: false,
    error: null,
    acknowledged: {},
  });
});

describe('P6.6 compact outbox panel', () => {
  it('describes each operation without ever rendering the queued payload', async () => {
    const withPayload = {
      ...op({ opId: 7, state: 'failed', errorMessage: 'The provider rejected this message' }),
      rawMime: 'From: me@x.com\r\nSubject: secret-payload-marker',
    } as unknown as OutboxOp;
    mocks.api.outbox_list.mockResolvedValue(page([withPayload]));

    render(<OutboxPanel />);

    const row = await screen.findByTestId('outbox-row-7');
    expect(row).toHaveTextContent('ben@y.org');
    expect(row).toHaveTextContent('Quarterly report');
    expect(row).toHaveTextContent('Send');
    expect(row).toHaveTextContent('The provider rejected this message');
    expect(document.body.textContent).not.toContain('secret-payload-marker');
    expect(document.body.textContent).not.toContain('rawMime');
  });

  it('names each state and the schedule a pending operation is waiting for', async () => {
    const future = Date.now() + 3_600_000;
    mocks.api.outbox_list.mockResolvedValue(
      page([
        op({ opId: 1, state: 'pending' }),
        op({ opId: 2, state: 'pending', scheduledAt: future }),
        op({ opId: 3, state: 'inflight' }),
        op({ opId: 4, state: 'uncertain', requiresDuplicateAck: true }),
        op({ opId: 5, state: 'failed', errorMessage: 'Rejected' }),
      ]),
    );

    render(<OutboxPanel />);

    expect(await screen.findByTestId('outbox-state-1')).toHaveTextContent('Queued');
    expect(screen.getByTestId('outbox-state-2')).toHaveTextContent('Scheduled');
    expect(screen.getByTestId('outbox-state-3')).toHaveTextContent('Sending');
    expect(screen.getByTestId('outbox-state-4')).toHaveTextContent('Unconfirmed');
    expect(screen.getByTestId('outbox-state-5')).toHaveTextContent('Failed');
    expect(screen.getByTestId('outbox-row-2')).toHaveTextContent('Scheduled for');
  });

  it('keeps a 10,000-operation queue to one page of rows', async () => {
    const ops = Array.from({ length: OUTBOX_PAGE_LIMIT }, (_, i) => op({ opId: i + 1 }));
    mocks.api.outbox_list.mockResolvedValue(page(ops, { total: 10_000, nextCursor: '50' }));

    render(<OutboxPanel />);

    await waitFor(() =>
      expect(mocks.api.outbox_list).toHaveBeenCalledWith({
        accountIds: ['acc-a'],
        states: ['pending', 'inflight', 'uncertain', 'failed'],
        cursor: null,
        limit: OUTBOX_PAGE_LIMIT,
      }),
    );
    const rows = await screen.findAllByTestId(/^outbox-row-/);
    expect(rows).toHaveLength(OUTBOX_PAGE_LIMIT);
    expect(screen.getByText('Showing 50 of 10,000')).toBeTruthy();
  });

  it('refuses to retry an uncertain send until the duplicate risk is acknowledged', async () => {
    mocks.api.outbox_list.mockResolvedValue(
      page([op({ opId: 9, state: 'uncertain', requiresDuplicateAck: true })]),
    );
    mocks.api.outbox_retry.mockResolvedValue({ ...op({ opId: 9, state: 'pending' }) });

    render(<OutboxPanel />);

    const retry = await screen.findByTestId('outbox-retry-9');
    const ack = screen.getByTestId('outbox-ack-9');
    expect(screen.getByText('Retry sending — may duplicate')).toBeTruthy();
    expect(retry).toBeDisabled();

    fireEvent.click(retry);
    expect(mocks.api.outbox_retry).not.toHaveBeenCalled();

    fireEvent.click(ack);
    await waitFor(() => expect(retry).not.toBeDisabled());
    fireEvent.click(retry);
    await waitFor(() =>
      expect(mocks.api.outbox_retry).toHaveBeenCalledWith({
        opId: 9,
        acknowledgeDuplicateRisk: true,
      }),
    );
  });

  it('retries a failed operation without asking for an acknowledgement', async () => {
    mocks.api.outbox_list.mockResolvedValue(
      page([op({ opId: 4, state: 'failed', errorMessage: 'Rejected' })]),
    );
    mocks.api.outbox_retry.mockResolvedValue(op({ opId: 4, state: 'pending' }));

    render(<OutboxPanel />);

    fireEvent.click(await screen.findByTestId('outbox-retry-4'));
    await waitFor(() =>
      expect(mocks.api.outbox_retry).toHaveBeenCalledWith({
        opId: 4,
        acknowledgeDuplicateRisk: false,
      }),
    );
    expect(screen.queryByTestId('outbox-ack-4')).toBeNull();
  });
});
