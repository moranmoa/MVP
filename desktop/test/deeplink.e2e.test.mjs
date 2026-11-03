// mvs:// links end to end. A valid invite, on the first launch's command line
// or from a second instance, reaches the page as a `link.open` event, and
// NOTHING is sent to the core: joinServer happens only when the UI calls it
// (the user pressed Join). Hostile links reach nothing. The fake core logs
// every command main sends it.
import test, { after, before } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { _electron as electron } from 'playwright-core';

const require = createRequire(import.meta.url);
const ELECTRON = require('electron');
const FAKE_CORE = fileURLToPath(new URL('./fake-core.js', import.meta.url));
const FIXTURE_UI = fileURLToPath(new URL('./fixtures/link-ui', import.meta.url));
const A = `mvs://join/${'a'.repeat(64)}/${'b'.repeat(64)}/${'e'.repeat(64)}`;
const B = `mvs://join/${'c'.repeat(64)}/${'d'.repeat(64)}/${'f'.repeat(64)}`;
const env = { ...process.env, MVS_CORE_PATH: FAKE_CORE };

let app;
let page;
let log;
const commands = () => (existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)) : []);
const joins = () => commands().filter((c) => c.cmd === 'joinServer').map((c) => c.args.link);
const seen = () => page.evaluate(() => window.__links.slice());
const until = async (cond, ms = 15_000) => {
  const end = Date.now() + ms;
  while (!(await cond())) {
    if (Date.now() > end) return false;
    await new Promise((r) => setTimeout(r, 100));
  }
  return true;
};
const launchSecond = async (arg) => {
  const p = spawn(ELECTRON, ['.', '--', arg], { env, stdio: 'ignore' });
  return new Promise((r) => p.on('exit', r));
};

before(async () => {
  // A stand-in UI (test/fixtures/link-ui) that subscribes a second after load,
  // like a UI that mounts late, and records every link.open. The real UI has
  // its own listener, which would race the test for the first link.
  // Upper case and a trailing slash: the shell must canonicalise.
  app = await electron.launch({ args: ['.', '--', `${A.toUpperCase()}/`], env: { ...env, MVS_UI_DIR: FIXTURE_UI } });
  page = await app.firstWindow();
  await page.waitForFunction(() => window.__subscribed === true);
  log = join(await app.evaluate(({ app: a }) => a.getPath('userData')), 'core', 'commands.log');
});

after(async () => {
  await app?.close();
});

test('a link on the first command line reaches the page, canonical', async () => {
  assert.ok(await until(async () => (await seen()).length > 0), 'link.open not delivered');
  assert.deepEqual(await seen(), [{ link: A }]);
});

test('a second launch hands its link to the running app and exits', async () => {
  assert.equal(await launchSecond(B), 0);
  assert.ok(await until(async () => (await seen()).length > 1), 'second link not delivered');
  assert.deepEqual(await seen(), [{ link: A }, { link: B }]);
});

test('hostile links reach nothing', async () => {
  for (const bad of ['mvs://join/xyz', `${A}?--inspect`, 'mvs://evil/../../etc', `--inspect ${A}`]) await launchSecond(bad);
  await new Promise((r) => setTimeout(r, 1500));
  assert.deepEqual(await seen(), [{ link: A }, { link: B }]);
  assert.equal(await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().length), 1);
});

test('no link ever causes a joinServer by itself', () => {
  assert.deepEqual(joins(), []);
});

test('joinServer happens only when the UI calls it (the user pressed Join)', async () => {
  await page.evaluate((link) => window.mvs.invoke('joinServer', [link]), A);
  assert.deepEqual(joins(), [A]);
});
