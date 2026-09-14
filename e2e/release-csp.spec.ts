import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { expect, test } from './fixture/test';
import { E2E_ENTRY } from './fixture/entry';

const config = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));

test('release CSP: email styles and sizing survive the packaged app policy', async ({ page, app }) => {
  await page.route(`**${E2E_ENTRY}`, async (route) => {
    const response = await route.fetch();
    const body = await response.text();
    // Tauri allows bundled bootstrap scripts by hash and appends style nonces.
    // Reproduce both without granting the dynamically created frame script access.
    const hashes = [...body.matchAll(/<script>([\s\S]*?)<\/script>/g)]
      .map((match) => `'sha256-${createHash('sha256').update(match[1]).digest('base64')}'`)
      .join(' ');
    const policy = config.app.security.csp
      .replace("script-src 'self'", `script-src 'self' ${hashes}`)
      .replace("style-src 'self'", "style-src 'self' 'nonce-release-style'");
    await route.fulfill({
      response,
      body,
      headers: { ...response.headers(), 'content-security-policy': policy },
    });
  });
  await app.gotoApp();
  await app.ready();
  const element = page.locator('iframe[title="Email"]').first();
  const frame = await (await element.elementHandle())!.contentFrame();
  await frame!.evaluate(() => {
    document.body.innerHTML += '<div style="height:1200px">Release sizing regression</div>';
  });
  await expect.poll(async () => (await element.boundingBox())!.height).toBeGreaterThan(1200);
  const metrics = await frame!.evaluate(() => ({
    font: getComputedStyle(document.body).fontFamily,
    content: document.documentElement.scrollHeight,
    viewport: document.documentElement.clientHeight,
  }));
  expect(metrics.font).not.toContain('Times');
  expect(metrics.content).toBeLessThanOrEqual(metrics.viewport + 2);
  await frame!.evaluate(() => {
    document.body.innerHTML = `<style>body { font-family: Georgia; background: rgb(244, 240, 232); color: rgb(35, 45, 55); }</style><h1>Authored newsletter</h1><details open><summary>Quoted history</summary><div style="height:1800px">Older message</div></details>`;
  });
  await expect.poll(async () => (await element.boundingBox())!.height).toBeGreaterThan(1800);
  expect(
    await frame!.evaluate(() => {
      const css = getComputedStyle(document.body);
      return { font: css.fontFamily, background: css.backgroundColor, color: css.color };
    }),
  ).toEqual({ font: 'Georgia', background: 'rgb(244, 240, 232)', color: 'rgb(35, 45, 55)' });
  await frame!.locator('summary').click();
  await expect.poll(async () => (await element.boundingBox())!.height).toBeLessThan(200);
  await page.setViewportSize({ width: 1100, height: 720 });
  await expect
    .poll(async () =>
      frame!.evaluate(() => document.documentElement.scrollHeight - document.documentElement.clientHeight),
    )
    .toBeLessThanOrEqual(2);
});
