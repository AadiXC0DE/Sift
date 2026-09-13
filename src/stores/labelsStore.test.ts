import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Label } from '../app/ipc/types';
import { labelKey, resetLabelIndex, useLabels } from './labelsStore';

const labelsList = vi.fn<(accountId: string) => Promise<Label[]>>();
vi.mock('../app/ipc/commands', () => ({
  api: { labels_list: (accountId: string) => labelsList(accountId) },
}));
vi.mock('../app/ipc/events', () => ({
  on: () => Promise.resolve(() => {}),
}));

function label(account_id: string, id: string, name: string): Label {
  return {
    account_id,
    id,
    name,
    kind: 'user',
    color_bg: null,
    color_fg: null,
    visible: true,
    unread_count: 0,
    total_count: 0,
    sort_order: 0,
  };
}

describe('P3.6 label names are indexed per account', () => {
  beforeEach(() => {
    labelsList.mockReset();
    resetLabelIndex();
  });

  it('keeps labels of the same name in different accounts apart', () => {
    // Both accounts own a label called "Client Work" with different ids; the
    // index must never resolve one account's row to the other's label.
    useLabels.getState().apply('acc-a', [label('acc-a', 'Label_A_Client', 'Client Work')]);
    useLabels.getState().apply('acc-b', [label('acc-b', 'Label_B_Client', 'Client Work')]);

    const names = useLabels.getState().names;
    expect(names[labelKey('acc-a', 'Label_A_Client')]).toBe('Client Work');
    expect(names[labelKey('acc-b', 'Label_B_Client')]).toBe('Client Work');
    expect(names[labelKey('acc-a', 'Label_B_Client')]).toBeUndefined();
    expect(names[labelKey('acc-b', 'Label_A_Client')]).toBeUndefined();
  });

  it('fetches each account at most once and indexes what came back', async () => {
    labelsList.mockImplementation(async (accountId) => [label(accountId, `${accountId}:l1`, 'Receipts')]);

    useLabels.getState().ensure(['acc-a', 'acc-b']);
    useLabels.getState().ensure(['acc-a', 'acc-b']);
    await vi.waitFor(() =>
      expect(useLabels.getState().names[labelKey('acc-b', 'acc-b:l1')]).toBe('Receipts'),
    );

    expect(labelsList).toHaveBeenCalledTimes(2);
    expect(labelsList.mock.calls.map((c) => c[0]).sort()).toEqual(['acc-a', 'acc-b']);
  });
});
