import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import type { RefObject } from 'react';

const webview = vi.hoisted(() => ({
  handler: null as null | ((event: { payload: unknown }) => void),
  unlisten: vi.fn(),
  onDragDropEvent: vi.fn(),
}));

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: webview.onDragDropEvent }),
}));

import { useNativeDropListener } from './attachDrop';

beforeEach(() => {
  vi.clearAllMocks();
  webview.handler = null;
  webview.onDragDropEvent.mockImplementation((h: (event: { payload: unknown }) => void) => {
    webview.handler = h;
    return Promise.resolve(webview.unlisten);
  });
});

function makeTarget(): { ref: RefObject<HTMLElement | null>; el: HTMLElement } {
  const el = document.createElement('div');
  el.getBoundingClientRect = () =>
    ({ left: 0, top: 0, right: 100, bottom: 100, width: 100, height: 100, x: 0, y: 0 }) as DOMRect;
  return { ref: { current: el }, el };
}

describe('P2.5 native drag/drop registration', () => {
  it('listens only while mounted and unlistens on close', async () => {
    const { ref } = makeTarget();
    const onDrop = vi.fn();
    const { unmount } = renderHook(() => useNativeDropListener(ref, onDrop));

    await waitFor(() => expect(webview.onDragDropEvent).toHaveBeenCalledTimes(1));
    unmount();
    expect(webview.unlisten).toHaveBeenCalledTimes(1);
  });

  it('accepts a drop inside the composer and ignores one outside', async () => {
    const { ref } = makeTarget();
    const onDrop = vi.fn();
    renderHook(() => useNativeDropListener(ref, onDrop));
    await waitFor(() => expect(webview.handler).not.toBeNull());

    webview.handler!({
      payload: { type: 'drop', paths: ['/Users/me/Report.pdf'], position: { x: 50, y: 50 } },
    });
    expect(onDrop).toHaveBeenCalledWith(['/Users/me/Report.pdf']);

    onDrop.mockClear();
    webview.handler!({
      payload: { type: 'drop', paths: ['/Users/me/Other.pdf'], position: { x: 900, y: 50 } },
    });
    webview.handler!({ payload: { type: 'over', position: { x: 50, y: 50 } } });
    expect(onDrop).not.toHaveBeenCalled();
  });
});
