import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render } from '@testing-library/react';
import { QueryErrorState } from './ThreadList';

describe('P3.3 query error state', () => {
  it('offers Retry and never claims the inbox is caught up', () => {
    const onRetry = vi.fn();
    const { getByRole, queryByText, getByText } = render(
      <QueryErrorState message="imap unavailable" onRetry={onRetry} />,
    );
    expect(getByText(/imap unavailable/)).toBeInTheDocument();
    expect(queryByText(/caught up/i)).toBeNull();
    fireEvent.click(getByRole('button', { name: /retry/i }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});
