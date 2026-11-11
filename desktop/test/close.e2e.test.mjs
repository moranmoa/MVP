// QA's v0.4.2 voice run: after the renderer was killed during a call,
// app.close() never returned, because main held the close for a page that
// could no longer answer (#358). A quit, or a dead page, must close at once.
// Needs a display: run in Docker with `docker compose run --rm test`.
import test, { after } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { _electron as electron } from 'playwright-core';

const FAKE_CORE = fileURLToPath(new URL('./fake-core.js', import.meta.url));
const PROFILE = mkdtempSync(join(tmpdir(), 'mvs-e2e-close-'));

after(() => rmSync(PROFILE, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }));

test('in a call with the renderer killed, closing the app returns at once', async () => {
  const app = await electron.launch({ args: ['.'], env: { ...process.env, MVS_DEMO: '1', MVS_CORE_PATH: FAKE_CORE, MVS_TEST_USER_DATA: PROFILE } });
  const page = await app.firstWindow();
  await page.waitForFunction(() => document.querySelector('#root')?.children.length > 0, null, { timeout: 30_000 });
  const joined = await page.evaluate(() => window.mvs.invoke('joinCall', ['c'.repeat(64)]));
  assert.ok(joined.ok, JSON.stringify(joined));
  await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].webContents.forcefullyCrashRenderer());
  const closed = await Promise.race([app.close().then(() => 'closed'), new Promise((r) => setTimeout(() => r('hung'), 15_000))]);
  assert.equal(closed, 'closed');
});
