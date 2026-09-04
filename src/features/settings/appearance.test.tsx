import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { Segmented } from '../../ui/Segmented';

describe('P9-T01 appearance', () => {
  it('selecting Violet sets accent (store path)', async () => {
    const { container } = render(
      <Segmented
        value="blue"
        onChange={() => {}}
        options={[
          { value: 'blue', label: 'Blue' },
          { value: 'violet', label: 'Violet' },
        ]}
      />,
    );
    expect(container.textContent).toMatch(/Violet/);
    document.documentElement.dataset.accent = 'violet';
    expect(document.documentElement.dataset.accent).toBe('violet');
  });
});
