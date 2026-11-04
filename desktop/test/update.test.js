// The shell's part of in-app update (src/update.js).
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { resolveUpdate, deriveRoot, coreArgs, reportHealthy, restartToUpdate } = require('../src/update');
const { toRequest } = require('../src/commands');

const ROOT = path.resolve('/apps/MVS');
const LAUNCHER = path.join(ROOT, 'mvs-launcher.exe');
const BASE = path.join(ROOT, 'MVS.exe');
const V2 = path.join(ROOT, 'versions', '0.2.0', 'MVS.exe');
const withLauncher = (p) => p === LAUNCHER;
const none = () => false;
const quiet = { warn() {} };

test('the root is derived for the base copy and for versions/<ver>', () => {
  assert.equal(deriveRoot({ execPath: BASE, platform: 'win32', exists: withLauncher }), ROOT);
  assert.equal(deriveRoot({ execPath: V2, platform: 'win32', exists: withLauncher }), ROOT);
  assert.equal(deriveRoot({ execPath: path.join(ROOT, 'versions', '10.0.12', 'MVS.exe'), platform: 'win32', exists: withLauncher }), ROOT);
  // Only names the updater can write (strict MAJOR.MINOR.PATCH).
  for (const odd of ['0.3.0-alpha.1', '01.2.0', '1.2', '1.2.0.0', 'v1.2.0']) {
    assert.equal(deriveRoot({ execPath: path.join(ROOT, 'versions', odd, 'MVS.exe'), platform: 'win32', exists: withLauncher }), null, odd);
  }
});

test('no launcher, odd folder names, or a dev checkout: updates off', () => {
  assert.equal(deriveRoot({ execPath: BASE, platform: 'win32', exists: none }), null);
  assert.equal(deriveRoot({ execPath: path.join(ROOT, 'versions', '..x', 'MVS.exe'), platform: 'win32', exists: withLauncher }), null);
  assert.equal(deriveRoot({ execPath: path.join(ROOT, 'other', '0.2.0', 'MVS.exe'), platform: 'win32', exists: withLauncher }), null);
  assert.equal(resolveUpdate({ packaged: false, execPath: BASE, argv: [], platform: 'win32', exists: withLauncher }), null);
});

test('a packaged run with the launcher has updates on, launcher next to the root', () => {
  const upd = resolveUpdate({ packaged: true, execPath: V2, argv: [V2, `--mvs-update-root=${ROOT}`], platform: 'win32', exists: withLauncher, ...quiet });
  assert.deepEqual(upd, { root: ROOT, launcher: LAUNCHER });
  // Started directly (no launcher argument): the derived root still holds.
  assert.deepEqual(resolveUpdate({ packaged: true, execPath: BASE, argv: [BASE], platform: 'win32', exists: withLauncher }), { root: ROOT, launcher: LAUNCHER });
  // Windows paths compare case-insensitively.
  assert.ok(resolveUpdate({ packaged: true, execPath: V2, argv: [`--mvs-update-root=${ROOT.toUpperCase()}\\`], platform: 'win32', exists: withLauncher }));
});

test('--mvs-update-root that names another folder turns updates off, with a warning', () => {
  const warnings = [];
  const upd = resolveUpdate({
    packaged: true,
    execPath: V2,
    argv: [`--mvs-update-root=${path.resolve('/evil')}`],
    platform: 'win32',
    exists: withLauncher,
    warn: (m) => warnings.push(m),
  });
  assert.equal(upd, null);
  assert.equal(warnings.length, 1);
  assert.ok(!warnings[0].includes('evil'), 'the given path is not echoed');
});

test('the core gets --update-root only when updates are on', () => {
  assert.deepEqual(coreArgs(null), []);
  assert.deepEqual(coreArgs({ root: ROOT, launcher: LAUNCHER }), ['--update-root', ROOT]);
});

test('markHealthy only after the core answers, and only with updates on', async () => {
  const calls = [];
  const ok = (cmd) => {
    calls.push(cmd);
    return Promise.resolve({ ok: {} });
  };
  assert.equal(await reportHealthy(null, ok, () => {}), false);
  assert.deepEqual(calls, []);
  assert.equal(await reportHealthy({ root: ROOT }, ok, () => {}), true);
  assert.deepEqual(calls, ['getAppInfo', 'markHealthy']);

  const down = (cmd) => {
    calls.push(cmd);
    return Promise.resolve({ error: { code: 'core-unavailable' } });
  };
  calls.length = 0;
  assert.equal(await reportHealthy({ root: ROOT }, down, () => {}), false);
  assert.deepEqual(calls, ['getAppInfo'], 'no markHealthy when the core is not up');
});

test('restartToUpdate: applyUpdate first, relaunch only when the core agrees', async () => {
  const calls = [];
  let relaunched = 0;
  const relaunch = () => relaunched++;
  const answer = (res) => (cmd) => {
    calls.push(cmd);
    return Promise.resolve(res);
  };
  assert.deepEqual(await restartToUpdate({ root: ROOT }, answer({ ok: { version: '1.3.0' } }), relaunch), { ok: null });
  assert.deepEqual(calls, ['applyUpdate']);
  assert.equal(relaunched, 1);
  // In a call, or nothing staged: the error goes back to the UI, no relaunch.
  for (const code of ['not-allowed', 'nothing-staged']) {
    assert.deepEqual(await restartToUpdate({ root: ROOT }, answer({ error: { code } }), relaunch), { error: { code } });
  }
  assert.equal(relaunched, 1);
  assert.deepEqual(await restartToUpdate(null, answer({ ok: {} }), relaunch), { error: { code: 'not-implemented' } });
  assert.equal(relaunched, 1);
});

test('the renderer cannot reach the update commands (GAP-17)', () => {
  for (const cmd of ['markHealthy', 'checkForUpdate', 'downloadUpdate', 'applyUpdate']) {
    assert.deepEqual(toRequest(cmd, []), { error: 'not-allowed' }, cmd);
  }
  assert.equal(toRequest('restartToUpdate', []).cmd, 'restartToUpdate');
});
