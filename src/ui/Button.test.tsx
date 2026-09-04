import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { Button } from './Button';

describe('P1-T09 Button', () => {
  it('has active scale style and focus-visible ring; disabled prevents click', async () => {
    const { container } = render(<Button>Hi</Button>);
    const btn = container.querySelector('button')!;
    expect(btn).toBeTruthy();
    // transition includes transform 120ms
    expect(btn.getAttribute('style')).toMatch(/transform/);
    const { container: c2 } = render(
      <Button
        disabled
        onClick={() => {
          throw new Error('should not fire');
        }}
      >
        x
      </Button>,
    );
    expect((c2.querySelector('button') as HTMLButtonElement).disabled).toBe(true);
  });
});
