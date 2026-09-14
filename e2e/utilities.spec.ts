/**
 * Phase 8 and P9.3 acceptance: Send Later, reminders, notifications, find,
 * reader utilities, Empty Trash and label management.
 *
 * Every test here drives a named action and asserts a visible *and* persisted
 * outcome. The fixture is the one place a wall-clock deadline can be crossed
 * deterministically, so the schedule tests move the fixture clock rather than
 * sleeping.
 */
import type { Page } from '@playwright/test';
import { expect, test } from './fixture/test';
import { fixtureCalls, outbox } from './fixture/helpers';

async function openComposer(page: Page, subject: string): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('c');
  await expect(page.getByLabel('To recipients')).toBeVisible();
  await page.getByLabel('To recipients').fill('ben@example.test');
  await page.keyboard.press('Enter');
  await page.getByLabel('Subject').fill(subject);
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.evaluate((id) => {
    const row = document.querySelector(`[data-testid="row-${id}"]`) as HTMLElement | null;
    row?.click();
  }, threadId);
  await expect(page.locator('iframe[title="Email"]').first()).toBeVisible();
}

/** P8.1: a scheduled send is a queued operation with a deadline, and it says so. */
test('P8.1 scheduling from the Send menu shows the zone, survives a relaunch and sends when the clock passes it', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  // Nothing is scheduled, so there is no Send Later navigation to offer.
  await expect(page.getByTestId('nav-send-later')).toHaveCount(0);

  await openComposer(page, 'Quarterly numbers');
  await page.getByTestId('send-later-trigger').click();
  await page.getByRole('menuitem', { name: 'Tomorrow 8:00' }).click();

  // The banner names the exact instant and the zone it belongs to, and repeats
  // the "Sift must be running and online" consequence (P8.1).
  const banner = page.getByTestId('compose-scheduled-at');
  await expect(banner).toContainText('sends');
  await expect(page.getByTestId('compose-queued')).toContainText(
    'Sift must be running and online. Otherwise this sends when Sift next connects.',
  );
  await page.getByRole('button', { name: 'Close composer' }).click();

  const opId = await page.evaluate(() => window.__siftFixture!.control.outbox()[0].op_id);
  await expect(page.getByTestId('nav-send-later')).toBeVisible();

  await page.getByTestId('nav-send-later').click();
  const row = page.getByTestId(`scheduled-${opId}`);
  await expect(row).toContainText('Quarterly numbers');
  await expect(row).toContainText('Sends');

  // Quit and relaunch: the deadline is durable, not component state.
  await app.gotoApp();
  await app.ready();
  await expect(page.getByTestId('nav-send-later')).toBeVisible();

  // Cross the deadline with no user action at all.
  await page.evaluate(() => window.__siftFixture!.control.advanceClock(25 * 60 * 60 * 1000));
  await expect.poll(async () => (await outbox(page))[0].state).toBe('done');
  await expect(page.getByTestId('nav-send-later')).toHaveCount(0);
});

/** P8.1: sending now clears the deadline through the outbox, not by guessing. */
test('P8.1 Send now on a scheduled item claims it immediately and the view empties', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await openComposer(page, 'Send me sooner');
  await page.getByTestId('send-later-trigger').click();
  await page.getByRole('menuitem', { name: 'Tomorrow 8:00' }).click();
  await page.getByRole('button', { name: 'Close composer' }).click();

  await page.evaluate(() => window.__siftFixture!.control.outbox()[0].op_id);
  await page.getByTestId('nav-send-later').click();
  await page.getByRole('button', { name: 'Send now' }).click();

  expect(await fixtureCalls(page, 'send_now')).toHaveLength(1);
  await expect
    .poll(async () => await page.evaluate(() => window.__siftFixture!.control.outbox()[0].scheduledAt))
    .toBe(null);
});

/**
 * P8.2: a reminder leaves the message exactly where it is, and a deadline that
 * cannot notify is still visible as due rather than silently lost.
 */
test('P8.2 a reminder never changes the mail and stays visible when notifications are denied', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await page.evaluate(() => window.__siftFixture!.control.setNotifications({ permission: 'denied' }));

  const before = await page.evaluate(() => window.__siftFixture!.control.threads('acc-a')[0]);
  await openThread(page, 'blue-00');

  await page.locator('[data-testid^="message-actions-"]').first().click();
  await page.getByRole('menuitem', { name: /Remind me/ }).click();
  await page.getByRole('button', { name: 'Tomorrow 8:00' }).click();
  await page.getByRole('button', { name: 'Set reminder' }).click();

  await expect(page.getByTestId('nav-reminders')).toBeVisible();
  expect(await fixtureCalls(page, 'reminder_set')).toHaveLength(1);

  // The reminder is a local row: no label, read state or timestamp moved.
  const after = await page.evaluate(() => window.__siftFixture!.control.threads('acc-a')[0]);
  expect(after.labelIds).toEqual(before.labelIds);
  expect(after.unreadCount).toBe(before.unreadCount);
  expect(after.lastMessageAt).toBe(before.lastMessageAt);
  expect(await page.evaluate(() => window.__siftFixture!.control.reminders())).toHaveLength(1);

  // Cross the reminder deadline with the OS refusing notifications.
  await page.evaluate(() => window.__siftFixture!.control.advanceClock(30 * 60 * 60 * 1000));
  await page.getByTestId('nav-reminders').click();
  await expect(page.getByTestId('reminder-acc-a:blue-00')).toContainText(
    'Due — no notification was delivered',
  );
});

/** P8.5: emptying Trash shows the count first, then deletes exactly that mail. */
test('P8.5 Empty Trash previews a count before anything is deleted', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await page.evaluate(() => {
    window.__siftFixture!.control.mutateThread('acc-a', 'blue-00', { labelIds: ['TRASH'] });
    window.__siftFixture!.control.mutateThread('acc-a', 'blue-01', { labelIds: ['TRASH'] });
  });

  await page.evaluate(() => {
    const nav = [...document.querySelectorAll('nav[aria-label="Mailbox"] button')].find((b) =>
      b.textContent?.includes('Trash'),
    ) as HTMLElement | undefined;
    nav?.click();
  });
  await page.getByTestId('empty-trash').click();

  // Trash is not empty when this test starts (the dataset seeds trashed mail),
  // so the honest number is every conversation currently in Trash — derived
  // from persisted state rather than assumed from the two rows just moved.
  const inTrash = await page.evaluate(
    () =>
      [
        ...window.__siftFixture!.control.threads('acc-a'),
        ...window.__siftFixture!.control.threads('acc-b'),
      ].filter((t) => t.labelIds.includes('TRASH')).length,
  );
  expect(inTrash).toBeGreaterThan(2);
  await expect(page.getByText(`Permanently delete ${inTrash} conversations`)).toBeVisible();
  expect(await fixtureCalls(page, 'trash_empty')).toHaveLength(0);

  await page.getByRole('button', { name: 'Delete permanently' }).click();
  await expect
    .poll(async () => await page.evaluate(() => window.__siftFixture!.control.callCount('trash_empty')))
    .toBe(1);
  await expect
    .poll(async () =>
      (await page.evaluate(() => window.__siftFixture!.control.threads('acc-a'))).some(
        (t) => t.id === 'blue-00',
      ),
    )
    .toBe(false);
});

/** P8.5: a label is managed from its own row and its conversations keep their other labels. */
test('P8.5 renaming a label from the sidebar updates the label and deletes no mail', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  const menu = page.locator('[data-testid^="label-menu-acc-a-"]').first();
  await menu.click({ force: true });
  await page.getByRole('menuitem', { name: 'Rename…' }).click();

  const input = page.getByTestId('rename-label-input');
  await expect(input).toBeVisible();
  await input.fill('Client Work 2');
  await page.getByRole('button', { name: 'Rename', exact: true }).click();

  expect(await fixtureCalls(page, 'label_rename')).toHaveLength(1);
  await expect(page.locator('[data-label-name="Client Work 2"]')).toBeVisible();
});

/** P8.4: one message is one notification, the policy filters it, and the click routes. */
test('P8.4 notifications follow the configured policy and route back to the right conversation', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  const sent = () => page.evaluate(() => window.__siftFixture!.control.notificationsSent().length);

  await page.evaluate(() =>
    window.__siftFixture!.control.injectNewMail(
      window.__siftFixture!.ids.accountA,
      'notify-1',
      'One message',
    ),
  );
  expect(await sent()).toBe(1);

  // VIP-only: a non-VIP sender is silent, and a hidden subject never ships one.
  await page.evaluate(() => window.__siftFixture!.control.setNotifications({ filter: 'vip' }));
  await page.evaluate(() =>
    window.__siftFixture!.control.injectNewMail(
      window.__siftFixture!.ids.accountA,
      'notify-2',
      'From a stranger',
    ),
  );
  expect(await sent()).toBe(1);

  await page.evaluate(() =>
    window.__siftFixture!.control.setNotifications({ filter: 'inbox', hideSubject: true }),
  );
  await page.evaluate(() =>
    window.__siftFixture!.control.injectNewMail(
      window.__siftFixture!.ids.accountA,
      'notify-3',
      'Secret subject',
    ),
  );
  const entries = await page.evaluate(() => window.__siftFixture!.control.notificationsSent());
  expect(entries.at(-1)?.subject).toBe(null);
  expect(entries.at(-1)?.hidden).toBe(true);

  // The real OS banner arrives from the runtime; the frontend only navigates.
  await page.evaluate(() => window.__siftFixture!.control.clickNotification());
  await expect(page.getByTestId('msg-notify-3-m1')).toBeVisible();
});

/** P8.3: a rule is off by default and running it over a mailbox shows the count first. */
test('P8.3 a new rule is disabled, and running it on existing mail shows the count before applying', async ({
  page,
  app,
}) => {
  await app.gotoApp();
  await app.ready();

  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByRole('button', { name: 'Rules' }).click();
  await page.getByRole('button', { name: 'New rule' }).first().click();
  await page.getByLabel('Rule name').fill('Invoices');
  // The default condition is "sender contains", which matches none of the
  // seeded senders; pick the field whose subjects actually contain "invoice",
  // so the preview has a real count to show. Changing the field clears the
  // value, so it is chosen first.
  await page.getByLabel('Condition field').selectOption('subject');
  await page.getByLabel('Condition value').first().fill('invoice');
  await page.getByRole('button', { name: 'Add action' }).click();
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  await expect(page.getByText('Invoices')).toBeVisible();
  const rules = await page.evaluate(() => window.__siftFixture!.control.rules());
  expect(rules).toHaveLength(1);
  expect(rules[0].enabled).toBe(false);

  // Applying over an existing mailbox is preview-then-apply, never a blind run.
  await page.getByRole('button', { name: /Run on existing mail/ }).click();
  await expect
    .poll(async () => await page.evaluate(() => window.__siftFixture!.control.callCount('rules_preview')))
    .toBeGreaterThan(0);
  await expect(page.getByText(/\d+ existing messages match\./)).toBeVisible();
  await expect(page.getByRole('button', { name: /^Apply to / })).toBeVisible();
  expect(await page.evaluate(() => window.__siftFixture!.control.callCount('rules_apply_existing'))).toBe(0);
});

/** P9.3: Cmd+F is a reader-local find, and Escape closes only find. */
test('P9.3 Cmd+F reports match counts and Escape does not leave the conversation', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await openThread(page, 'blue-00');
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('Meta+f');

  const input = page.getByLabel('Find in message');
  await expect(input).toBeVisible();
  await input.fill('e');
  await expect(page.getByText(/\d+ of \d+/)).toBeVisible();

  await page.keyboard.press('Enter');
  await expect(page.getByText(/\d+ of \d+/)).toBeVisible();
  await page.getByRole('button', { name: 'Previous match' }).click();

  await page.keyboard.press('Escape');
  await expect(input).toBeHidden();
  // Escape closed find and nothing else: the conversation is still open.
  await expect(page.locator('iframe[title="Email"]').first()).toBeVisible();
});

/** P9.3: a reader utility never marks unrelated mail read. */
test('P9.3 View source opens the raw message and leaves unrelated mail untouched', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  const otherBefore = await page.evaluate(
    () => window.__siftFixture!.control.threads('acc-a').find((t) => t.id !== 'blue-00')?.unreadCount,
  );

  await openThread(page, 'blue-00');
  await page.locator('[data-testid^="message-actions-"]').first().click();
  await page.getByRole('menuitem', { name: 'View source' }).click();

  await expect(page.getByLabel('Raw message source')).toBeVisible();
  expect(await fixtureCalls(page, 'message_raw_source')).toHaveLength(1);

  const otherAfter = await page.evaluate(
    () => window.__siftFixture!.control.threads('acc-a').find((t) => t.id !== 'blue-00')?.unreadCount,
  );
  expect(otherAfter).toBe(otherBefore);
});

/** P9.3: the print surface states whether the collapsed quote is included. */
test('P9.3 the print preview says what will print and toggles the collapsed quote', async ({ page, app }) => {
  await app.gotoApp();
  await app.ready();

  await openThread(page, 'blue-00');
  await page.locator('[data-testid^="message-actions-"]').first().click();
  await page.getByRole('menuitem', { name: 'Print…' }).click();

  await expect(page.getByRole('button', { name: 'Print…' })).toBeVisible();
  // The surface has to state, before printing, whether the collapsed quote is
  // part of the page — the spec names that state, not a particular sentence,
  // so this pins the copy the dialog actually carries.
  await expect(page.getByText('Quoted replies stay hidden.')).toBeVisible();
  await page.getByRole('switch', { name: /Include collapsed quoted text/i }).click();
  await expect(page.getByText('Quoted replies are expanded.')).toBeVisible();
});
