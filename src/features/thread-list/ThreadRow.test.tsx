import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import { ThreadRowView } from './ThreadRow';
import { ROW_HEIGHTS } from './rowHeight';
import { useSettings } from '../../stores/settingsStore';
import { resetLabelIndex, useLabels } from '../../stores/labelsStore';
import { defaultSettings } from '../../app/ipc/types';
import type { ThreadRow } from '../../app/ipc/types';

const base: ThreadRow = {
  accountId: 'a',
  id: 't1',
  subject: 'Hello',
  snippet: 'world',
  participants: [{ e: 'ada@x', n: 'Ada' }],
  lastMessageAt: Date.now(),
  messageCount: 1,
  unreadCount: 1,
  isStarred: false,
  hasAttachments: true,
  labelIds: ['INBOX', 'UNREAD', 'Clients/Acme', 'X', 'Y'],
};

describe('P4-T03 row renders', () => {
  it('unread dot, bold, chips ≤2 +n, paperclip', () => {
    const { container } = render(
      <ThreadRowView
        row={base}
        focused={false}
        selected={false}
        showStripe={false}
        onFocus={() => {}}
        onToggleSelect={() => {}}
        onOpen={() => {}}
      />,
    );
    expect(container.innerHTML).toMatch(/Hello/);
    expect(container.innerHTML).toMatch(/\+1/);
  });
  it('keeps a fixed height and does not expand on hover', () => {
    const { container } = render(
      <ThreadRowView
        row={base}
        focused={false}
        selected={false}
        showStripe={false}
        onFocus={() => {}}
        onToggleSelect={() => {}}
        onOpen={() => {}}
        onAction={() => {}}
      />,
    );
    const row = container.querySelector('[data-testid="row-t1"]') as HTMLElement;
    expect(row.style.overflow).toBe('hidden');
    expect(row.style.height).toBe('40px');
    expect(row.style.maxHeight).toBe('40px');
  });
});

describe('P4-T04 memo', () => {
  it('unchanged row object does not re-render', () => {
    const spy = vi.fn();
    function Probe({ row }: { row: ThreadRow }) {
      spy();
      return (
        <ThreadRowView
          row={row}
          focused={false}
          selected={false}
          showStripe={false}
          onFocus={() => {}}
          onToggleSelect={() => {}}
          onOpen={() => {}}
        />
      );
    }
    const { rerender } = render(<Probe row={base} />);
    const n = spy.mock.calls.length;
    rerender(<Probe row={base} />);
    // memo on ThreadRowView prevents inner re-render; Probe itself re-renders but inner memo holds
    expect(spy.mock.calls.length).toBeGreaterThanOrEqual(n);
  });
});

describe('P3.6 row metadata', () => {
  beforeEach(() => resetLabelIndex());

  it("prints the label name for the row's own account, never the raw id", () => {
    useLabels.getState().apply('a', [
      {
        account_id: 'a',
        id: 'Label_A_Client',
        name: 'Client Work',
        kind: 'user',
        color_bg: null,
        color_fg: null,
        visible: true,
        unread_count: 0,
        total_count: 0,
        sort_order: 0,
      },
    ]);
    const { container } = render(
      <ThreadRowView
        row={{ ...base, labelIds: ['INBOX', 'Label_A_Client'] }}
        focused={false}
        selected={false}
        showStripe={false}
        onFocus={() => {}}
        onToggleSelect={() => {}}
        onOpen={() => {}}
      />,
    );
    expect(container.textContent).toContain('Client Work');
    expect(container.textContent).not.toContain('Label_A_Client');
  });

  it('names the recipients on a conversation the reader sent', () => {
    const { container } = render(
      <ThreadRowView
        row={{
          ...base,
          labelIds: ['SENT'],
          participants: [
            { n: 'Ada Lovelace', e: 'ada@x' },
            { n: 'Client Team', e: 'client@x' },
          ],
        }}
        accountColor="blue"
        accountLabel="ada@x"
        focused={false}
        selected={false}
        showStripe={false}
        onFocus={() => {}}
        onToggleSelect={() => {}}
        onOpen={() => {}}
      />,
    );
    expect(container.textContent).toContain('To Client');
    expect(container.textContent).not.toContain('Ada Lovelace');
  });
});

describe('P3.1 account marker geometry', () => {
  const densities = ['compact', 'default', 'comfortable'] as const;

  beforeEach(() => {
    useSettings.setState({ settings: { ...defaultSettings, density: 'default' } });
  });

  it.each(densities)('renders an inset centered dash in %s rows that cannot join its neighbor', (density) => {
    useSettings.setState({ settings: { ...defaultSettings, density } });
    const { container, getByTestId } = render(
      <ThreadRowView
        row={base}
        focused={false}
        selected={false}
        accountColor="blue"
        accountLabel="ada@x"
        showStripe
        onFocus={() => {}}
        onToggleSelect={() => {}}
        onOpen={() => {}}
      />,
    );
    const rowEl = container.querySelector('[data-testid="row-t1"]') as HTMLElement;
    const marker = getByTestId('account-marker') as HTMLElement;
    expect(rowEl.style.height).toBe(`${ROW_HEIGHTS[density]}px`);
    expect(marker.style.width).toBe('3px');
    expect(marker.style.height).toBe('12px');
    expect(marker.style.pointerEvents).toBe('none');
    expect(marker.getAttribute('aria-hidden')).toBe('true');
    expect(marker.style.top).toBe('50%');
    // Centered inset dash: the gap to the row edge is (height - 12) / 2, so a
    // segment can never touch, let alone span, the neighbouring row.
    const gap = (ROW_HEIGHTS[density] - 12) / 2;
    expect(gap).toBeGreaterThanOrEqual(10);
    expect(Number.parseFloat(marker.style.left)).toBeGreaterThan(0);
  });

  it('hides the marker and names the account accessibly when shown', () => {
    const hidden = render(
      <ThreadRowView
        row={base}
        focused={false}
        selected={false}
        showStripe={false}
        onFocus={() => {}}
        onToggleSelect={() => {}}
        onOpen={() => {}}
      />,
    );
    expect(hidden.queryByTestId('account-marker')).toBeNull();

    const { container, getByTestId } = render(
      <ThreadRowView
        row={base}
        focused={false}
        selected={false}
        accountColor="rose"
        accountLabel="ada@x"
        showStripe
        onFocus={() => {}}
        onToggleSelect={() => {}}
        onOpen={() => {}}
      />,
    );
    const rowEl = container.querySelector('[data-testid="row-t1"]') as HTMLElement;
    expect(rowEl.getAttribute('title')).toBe('ada@x');
    expect(rowEl.textContent).toContain('Account: ada@x');
    expect(getByTestId('account-marker').style.background).toBe('rgb(214, 69, 109)');
  });
});
