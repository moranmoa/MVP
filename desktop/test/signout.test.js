'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { signOut, clearLogFiles } = require('../src/signout');

function harness({ confirm = true, res = { ok: null } } = {}) {
  const calls = { confirm: 0, request: 0, storage: 0, logs: 0 };
  return {
    calls,
    run: (args) =>
      signOut({
        args,
        confirmWithoutWords: async () => (calls.confirm++, confirm),
        request: async () => (calls.request++, res),
        clearStorage: async () => void calls.storage++,
        clearLogs: () => void calls.logs++,
      }),
  };
}

test('with the words: no native dialog; on ok the storage and logs are cleared', async () => {
  const h = harness();
  assert.deepStrictEqual(await h.run({ words: Array(12).fill('abandon') }), { ok: null });
  assert.deepStrictEqual(h.calls, { confirm: 0, request: 1, storage: 1, logs: 1 });
});

test('without the words: the native dialog first; declining never reaches the core', async () => {
  const h = harness({ confirm: false });
  assert.deepStrictEqual(await h.run({ withoutWords: true }), { error: { code: 'cancelled' } });
  assert.deepStrictEqual(h.calls, { confirm: 1, request: 0, storage: 0, logs: 0 });
  const yes = harness({ confirm: true });
  assert.deepStrictEqual(await yes.run({ withoutWords: true }), { ok: null });
  assert.deepStrictEqual(yes.calls, { confirm: 1, request: 1, storage: 1, logs: 1 });
});

test('a refusal from the core clears nothing', async () => {
  const h = harness({ res: { error: { code: 'keyring-unavailable' } } });
  assert.deepStrictEqual(await h.run({ words: Array(12).fill('abandon') }), { error: { code: 'keyring-unavailable' } });
  assert.deepStrictEqual(h.calls, { confirm: 0, request: 1, storage: 0, logs: 0 });
});

test('clearLogFiles deletes mvs.log and its rotated copy, and is fine when they are missing', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mvs-signout-'));
  fs.writeFileSync(path.join(dir, 'mvs.log'), 'x');
  fs.writeFileSync(path.join(dir, 'mvs.log.1'), 'y');
  fs.writeFileSync(path.join(dir, 'other.txt'), 'z');
  clearLogFiles(dir);
  assert.deepStrictEqual(fs.readdirSync(dir), ['other.txt']);
  clearLogFiles(dir);
  fs.rmSync(dir, { recursive: true, force: true });
});
