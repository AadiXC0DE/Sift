/**
 * Shared action helpers for specs. Each one drives a real user-visible control
 * and returns observable state, so tests read as user actions rather than as
 * fixture bookkeeping.
 */
import { expect, type Locator, type Page } from '@playwright/test';
import type { FixtureDraft } from './dataset';
import type { ThreadRow } from '../../src/app/ipc/types';

/** Sidebar entries append a count to their accessible name, so match the prefix. */
export async function sidebarView(page: Page, label: string): Promise<void> {
  await page
    .locator('nav[aria-label="Mailbox"]')
    .getByRole('button', { name: new RegExp(`^${label}\\b`) })
    .first()
    .click();
}

/** Label entries are keyed by their display name in the sidebar tree. */
export async function sidebarLabel(page: Page, name: string): Promise<void> {
  await page.locator(`nav[aria-label="Mailbox"] button[title="${name}"]`).first().click();
}

/** The 6px unread dot is the row's visible unread state. */
export async function showsUnreadDot(row: Locator): Promise<boolean> {
  return row.evaluate((el) =>
    [...el.querySelectorAll('span')].some((s) => {
      const r = s.getBoundingClientRect();
      return Math.round(r.width) === 6 && Math.round(r.height) === 6;
    }),
  );
}

/** The virtualizer's sized container height / row height = loaded row count. */
export async function loadedRowCount(page: Page, rowHeight = 40): Promise<number> {
  const height = await page
    .locator('[role="listbox"] [style*="position: relative"]')
    .first()
    .evaluate((el) => (el as HTMLElement).offsetHeight);
  return Math.round(height / rowHeight);
}

export async function listScrollTop(page: Page): Promise<number> {
  return page.locator('[role="listbox"]').evaluate((el) => el.scrollTop);
}

export async function topVisibleRowId(page: Page): Promise<string> {
  return page.locator('[role="listbox"]').evaluate((el) => {
    const rows = [...el.querySelectorAll<HTMLElement>('[data-testid^="row-"]')];
    const top = rows
      .map((r) => ({ id: r.dataset.testid ?? '', top: r.getBoundingClientRect().top }))
      .filter((r) => r.top >= (el.getBoundingClientRect().top ?? 0) - 1)
      .sort((a, b) => a.top - b.top)[0];
    return (top?.id ?? '').replace(/^row-/, '');
  });
}

/** Scroll the window until the thread is virtualized into the DOM. */
export async function revealRow(page: Page, threadId: string): Promise<Locator> {
  const loc = page.locator(`[data-testid="row-${threadId}"]`);
  for (let i = 0; i <= 20 && (await loc.count()) === 0; i++) await scrollListTo(page, i / 20);
  await expect(loc.first()).toHaveCount(1);
  return loc.first();
}

export async function scrollListTo(page: Page, fraction: number): Promise<void> {
  await page.locator('[role="listbox"]').evaluate((el, f) => {
    el.scrollTop = (el.scrollHeight - el.clientHeight) * f;
  }, fraction);
  await page.waitForTimeout(120);
}

export async function fixtureCalls(page: Page, cmd: string): Promise<Record<string, unknown>[]> {
  return page.evaluate((c) => {
    const log = window.__siftFixture!.control.calls() as { cmd: string; args?: Record<string, unknown> }[];
    return log.filter((e) => e.cmd === c).map((e) => e.args ?? {});
  }, cmd);
}

export async function persistedThread(page: Page, threadId: string): Promise<ThreadRow> {
  const rows = await page.evaluate(() => window.__siftFixture!.control.threads());
  const row = rows.find((r) => r.id === threadId);
  expect(row, `thread ${threadId} missing from the persisted dataset`).toBeTruthy();
  return row as ThreadRow;
}

export async function hasPersistedThread(page: Page, threadId: string): Promise<boolean> {
  const rows = await page.evaluate(() => window.__siftFixture!.control.threads());
  return rows.some((r) => r.id === threadId);
}

export async function drafts(page: Page): Promise<FixtureDraft[]> {
  return page.evaluate(() => window.__siftFixture!.control.drafts());
}

export async function outbox(page: Page): Promise<{ op_id: number; state: string; subject: string }[]> {
  return page.evaluate(() => window.__siftFixture!.control.outbox());
}

/** Press a list/global shortcut with focus parked outside any text field. */
export async function pressList(page: Page, key: string): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press(key);
}
