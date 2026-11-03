// mvs:// links end to end: on the first launch's command line and from a
// second instance, a valid invite becomes exactly one joinServer call; a
// hostile one reaches nothing. The fake core logs what main sends it.
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
const A = `mvs://join/${'a'.repeat(64)}/${'b'.repeat(64)}`;
const B = `mvs://join/${'c'.repeat(64)}/${'d'.repeat(64)}`;
const env = { ...process.env, MVS_CORE_PATH: FAKE_CORE };

let app;
let log;
const commands = () => (existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)) : []);
const joins = () => commands().filter((c) => c.cmd === 'joinServer').map((c) => c.args.link);
const until = async (cond, ms = 15_000) => {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) return false;
    await new Promise((r) => setTimeout(r, 100));
  }
  return true;
};

before(async () => {
  app = await electron.launch({ args: ['.', '--some-flag', A.toUpperCase() + '/'], env });
  await app.firstWindow();
  log = join(await app.evaluate(({ app: a }) => a.getPath('userData')), 'core', 'commands.log');
});

after(async () => {
  await app?.close();
});

test('an invite on the first command line becomes one joinServer, canonical', async () => {
  assert.ok(await until(() => joins().length > 0), 'joinServer was not called');
  assert.deepEqual(joins(), [A]);
});

test('a second launch hands its link to the running app and exits', async () => {
  const second = spawn(ELECTRON, ['.', B], { env, stdio: 'ignore' });
  const code = await new Promise((r) => second.on('exit', r));
  assert.equal(code, 0, 'the second instance quits');
  assert.ok(await until(() => joins().length > 1), 'second link not delivered');
  assert.deepEqual(joins(), [A, B]);
});

test('hostile links reach nothing', async () => {
  for (const bad of ['mvs://join/xyz', `mvs://join/${'a'.repeat(64)}/${'b'.repeat(64)}?--inspect`, 'mvs://evil/../../etc']) {
    const p = spawn(ELECTRON, ['.', bad], { env, stdio: 'ignore' });
    await new Promise((r) => p.on('exit', r));
  }
  await new Promise((r) => setTimeout(r, 1500));
  assert.deepEqual(joins(), [A, B]);
  assert.equal(await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().length), 1);
});
