// The sidecar manager (src/sidecar.js) against a fake core (fake-core.js).
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { Sidecar, minimalEnv, sanitizeError } = require('../src/sidecar');

const FAKE = path.join(__dirname, 'fake-core.js');
try {
  fs.chmodSync(FAKE, 0o755);
} catch {
  // read-only or foreign bind mount: git keeps the exec bit
}
const DATA = fs.mkdtempSync(path.join(os.tmpdir(), 'mvs-core-'));

function start(extra = {}) {
  const events = [];
  const logs = [];
  const core = new Sidecar({
    exe: FAKE,
    dataDir: DATA,
    onEvent: (name, payload) => events.push([name, payload]),
    log: (l) => logs.push(l),
    ...extra,
  }).start();
  return { core, events, logs };
}
const until = async (cond, ms = 10_000) => {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error('timed out');
    await new Promise((r) => setTimeout(r, 20));
  }
};

test('spawned by absolute path, no shell, minimal env, data dir argument', async () => {
  let seen;
  const spawnImpl = (exe, args, opts) => {
    seen = { exe, args, opts };
    return require('node:child_process').spawn(exe, args, opts);
  };
  process.env.NODE_OPTIONS_TEST_SECRET = 'leak';
  const { core } = start({ spawnImpl });
  const res = await core.request('getAppInfo', {});
  core.stop();
  assert.equal(seen.exe, FAKE);
  assert.equal(seen.opts.shell, false);
  assert.deepEqual(seen.args, ['--data-dir', DATA]);
  assert.deepEqual(res.ok.argv, ['--data-dir', DATA]);
  assert.ok(!res.ok.env.includes('NODE_OPTIONS_TEST_SECRET'));
  assert.ok(!res.ok.env.includes('NODE_OPTIONS'));
});

test('relative paths are refused', () => {
  assert.throws(() => new Sidecar({ exe: 'mvs-core', dataDir: DATA, onEvent() {} }), /absolute/);
  assert.throws(() => new Sidecar({ exe: FAKE, dataDir: 'data', onEvent() {} }), /absolute/);
});

test('minimalEnv keeps only the listed variables', () => {
  const env = minimalEnv({ HOME: '/h', NODE_OPTIONS: '--inspect', ELECTRON_RUN_AS_NODE: '1', LD_PRELOAD: 'x.so', DBUS_SESSION_BUS_ADDRESS: 'unix:x' });
  assert.deepEqual(env, { HOME: '/h', DBUS_SESSION_BUS_ADDRESS: 'unix:x' });
});

test('a request\'s events arrive before its response', async () => {
  const { core, events } = start();
  const res = await core.request('joinServer', { link: 'mvs://x' });
  core.stop();
  assert.deepEqual(res, { ok: { link: 'mvs://x', state: 'searching' } });
  assert.deepEqual(events, [['join.status', { link: 'mvs://x', state: 'searching' }]]);
});

test('errors reach the renderer with only code and integer positions', async () => {
  const { core } = start();
  const res = await core.request('checkRecoveryWords', { answers: {} });
  core.stop();
  assert.deepEqual(res, { error: { code: 'invalid-words', positions: [2, 5] } });
  assert.deepEqual(sanitizeError({ code: 'x'.repeat(100) }).code.length, 64);
});

test('stdout is never logged: words do not reach the log', async () => {
  const { core, logs } = start();
  const res = await core.request('getRecoveryWords', {});
  core.stop();
  assert.deepEqual(res.ok, ['fake', 'words', 'never', 'logged']);
  assert.ok(logs.some((l) => l.includes('fake core up')), 'stderr is logged');
  assert.ok(!logs.join('\n').includes('never'), 'stdout content must not be logged');
});

test('a request the core does not answer times out', async () => {
  const { core } = start({ timeouts: { default: 300 } });
  const res = await core.request('getStorageUsage', {});
  core.stop();
  assert.deepEqual(res, { error: { code: 'timeout' } });
});

test('a crash fails what is pending and the core is restarted', async () => {
  const { core } = start();
  const crashed = await core.request('clearCache', {});
  assert.deepEqual(crashed, { error: { code: 'core-restarted' } });
  await until(() => core.child);
  const again = await core.request('getIdentity', {});
  core.stop();
  assert.deepEqual(again, { ok: {} });
});

test('a line over 16 MiB from the core kills and restarts it', async () => {
  const { core, logs } = start();
  const res = await core.request('exportDiagnostics', {});
  assert.deepEqual(res, { error: { code: 'core-restarted' } });
  assert.ok(logs.some((l) => l.includes('over 16 MiB')));
  await until(() => core.child);
  assert.deepEqual(await core.request('getIdentity', {}), { ok: {} });
  core.stop();
});

test('a request over 1 MiB is not sent', async () => {
  const { core } = start();
  const res = await core.request('sendMessage', { body: 'x'.repeat(1024 * 1024) });
  core.stop();
  assert.deepEqual(res, { error: { code: 'too-large' } });
});

test('no core: requests answer core-unavailable', async () => {
  const core = new Sidecar({ exe: FAKE, dataDir: DATA, onEvent() {} });
  assert.deepEqual(await core.request('getIdentity', {}), { error: { code: 'core-unavailable' } });
});

// ---- status for the "core unavailable" screen (T8-01) ----------------------

const { EventEmitter } = require('node:events');

/** A child that fails the way `how` says, every time it is started. */
function failing(how) {
  return () => {
    const child = new EventEmitter();
    child.stdout = new EventEmitter();
    child.stderr = new EventEmitter();
    child.stdin = { write() {}, end() {} };
    child.kill = () => {};
    setImmediate(() => (how === 'spawn' ? child.emit('error', Object.assign(new Error('x'), { code: 'ENOENT' })) : child.emit('exit', 1, null)));
    return child;
  };
}

test('status goes up once the core answers, or says it is ready', async () => {
  const statuses = [];
  const { core } = start({ onStatus: (s) => statuses.push(s) });
  await core.request('getIdentity', {});
  core.stop();
  assert.deepEqual(statuses.at(-1), { state: 'up' });
  const ready = new Sidecar({ exe: FAKE, dataDir: DATA, onEvent() {} });
  ready.child = {};
  ready.onLine('{"type":"event","event":"x","payload":null}');
  assert.deepEqual(ready.status, { state: 'up' });
});

test('a core that cannot be started: down, start-failed, and requests say why', async () => {
  const statuses = [];
  const core = new Sidecar({ exe: FAKE, dataDir: DATA, onEvent() {}, onStatus: (s) => statuses.push(s), backoffBase: 50, spawnImpl: failing('spawn') }).start();
  await until(() => statuses.length > 0);
  assert.deepEqual(statuses[0], { state: 'down', detail: 'start-failed' });
  assert.deepEqual(await core.request('getIdentity', {}), { error: { code: 'core-unavailable', detail: 'start-failed' } });
  core.stop();
});

test('five crashes in a row without an answer: crash-loop', async () => {
  const statuses = [];
  const core = new Sidecar({ exe: FAKE, dataDir: DATA, onEvent() {}, onStatus: (s) => statuses.push(s), backoffBase: 1, spawnImpl: failing('exit') }).start();
  await until(() => statuses.some((s) => s.detail === 'crash-loop'));
  core.stop();
  assert.deepEqual(statuses[0], { state: 'down', detail: 'crashed' });
  assert.deepEqual(statuses.at(-1), { state: 'down', detail: 'crash-loop' });
});
