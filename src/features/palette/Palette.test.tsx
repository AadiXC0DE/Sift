import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { Palette } from './Palette';

describe('P8-T07 no transition + keep-open', () => {
  it('renders without transition classes', () => {
    const { container } = render(<Palette onClose={() => {}} onCompose={() => {}} onSettings={() => {}} />);
    expect(container.innerHTML).toMatch(/Type a command/);
    expect(container.innerHTML).not.toMatch(/transition-all/);
  });
});
