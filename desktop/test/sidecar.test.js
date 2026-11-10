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
  assert.throws(() => new Sidecar({ exe: process.execPath, script: 'fake-core.js', dataDir: DATA, onEvent() {} }), /absolute/);
});

test('a .js stand-in runs as exe <script> with ELECTRON_RUN_AS_NODE (checkouts on Windows)', async () => {
  let seen;
  const spawnImpl = (exe, args, opts) => {
    seen = { exe, args, opts };
    return require('node:child_process').spawn(exe, args, opts);
  };
  const core = new Sidecar({ exe: process.execPath, script: FAKE, dataDir: DATA, onEvent() {}, spawnImpl }).start();
  const res = await core.request('getAppInfo', {});
  core.stop();
  assert.equal(seen.exe, process.execPath);
  assert.deepEqual(seen.args, [FAKE, '--data-dir', DATA]);
  assert.equal(seen.opts.env.ELECTRON_RUN_AS_NODE, '1');
  assert.deepEqual(res.ok.argv, ['--data-dir', DATA]);
});

test('minimalEnv keeps only the listed variables', () => {
  const env = minimalEnv({ HOME: '/h', NODE_OPTIONS: '--inspect', ELECTRON_RUN_AS_NODE: '1', LD_PRELOAD: 'x.so', DBUS_SESSION_BUS_ADDRESS: 'unix:x' });
  assert.deepEqual(env, { HOME: '/h', DBUS_SESSION_BUS_ADDRESS: 'unix:x' });
});

test('MVS_DEBUG_LOG=1 gives the core a fixed sync/join filter, and nothing else does', () => {
  const filter = 'off,mvs::join=info,sync=debug';
  assert.equal(minimalEnv({ HOME: '/h', MVS_DEBUG_LOG: '1' }).RUST_LOG, filter);
  // The user's own RUST_LOG never reaches the core, with or without it.
  assert.equal(minimalEnv({ HOME: '/h', RUST_LOG: 'api=trace' }).RUST_LOG, undefined);
  assert.equal(minimalEnv({ HOME: '/h', RUST_LOG: 'api=trace', MVS_DEBUG_LOG: '1' }).RUST_LOG, filter);
  for (const v of ['0', 'true', 'yes', '', ' 1']) assert.equal(minimalEnv({ MVS_DEBUG_LOG: v }).RUST_LOG, undefined, v);
  assert.equal('MVS_DEBUG_LOG' in minimalEnv({ MVS_DEBUG_LOG: '1' }), false);
});

test('a sync debug line from the core reaches mvs.log with peer IDs and addresses redacted', () => {
  const { createLog } = require('../src/logfile');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mvs-debuglog-'));
  try {
    const log = createLog(dir);
    const peer = '12D3KooWBVzkbuxE3TA4LU7zEeMBM87G4Ns1KQoPdAc4MR8R3GTc';
    const server = 'ab'.repeat(32);
    log(`core: 2026-11-10T14:00:00Z DEBUG sync::node: dialling peer=${peer} address=/ip4/203.0.113.7/tcp/4001/p2p/${peer} server=${server}`);
    const text = fs.readFileSync(path.join(dir, 'mvs.log'), 'utf8');
    for (const secret of [peer, '203.0.113.7', server]) assert.ok(!text.includes(secret), text);
    assert.ok(text.includes('dialling'), text);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
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
function failing(how, code = 'EACCES') {
  return () => {
    const child = new EventEmitter();
    child.stdout = new EventEmitter();
    child.stderr = new EventEmitter();
    child.stdin = { write() {}, end() {} };
    child.kill = () => {};
    setImmediate(() => (how === 'spawn' ? child.emit('error', Object.assign(new Error('x'), { code })) : child.emit('exit', 1, null)));
    return child;
  };
}

test('a spawn that throws (Windows EFTYPE): down, start-failed, no exception to the caller', async () => {
  const statuses = [];
  const thrower = () => {
    throw Object.assign(new Error('spawn EFTYPE'), { code: 'EFTYPE' });
  };
  const core = new Sidecar({ exe: FAKE, dataDir: DATA, onEvent() {}, onStatus: (s) => statuses.push(s), backoffBase: 60_000, spawnImpl: thrower }).start();
  assert.deepEqual(statuses[0], { state: 'down', detail: 'start-failed' });
  assert.deepEqual(await core.request('getIdentity', {}), { error: { code: 'core-unavailable', detail: 'start-failed' } });
  core.stop();
});

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

test('an exe gone after launch (ENOENT, emitted or thrown): missing, never crash-loop', async () => {
  const thrower = () => {
    throw Object.assign(new Error('spawn ENOENT'), { code: 'ENOENT' });
  };
  for (const spawnImpl of [failing('spawn', 'ENOENT'), thrower]) {
    const statuses = [];
    let starts = 0;
    const counted = (...a) => {
      starts += 1;
      return spawnImpl(...a);
    };
    const core = new Sidecar({ exe: FAKE, dataDir: DATA, onEvent() {}, onStatus: (s) => statuses.push(s), backoffBase: 20, spawnImpl: counted }).start();
    // Ask between attempts (no start in flight): by the 7th the next retry is
    // over a second away. A request during an attempt gets core-restarted.
    await until(() => starts > 6 && core.child === null);
    assert.deepEqual(await core.request('getIdentity', {}), { error: { code: 'core-unavailable', detail: 'missing' } });
    core.stop();
    // Repeated identical statuses are not re-sent: one "missing", and no
    // crash-loop after more than five failed starts.
    assert.deepEqual(statuses, [{ state: 'down', detail: 'missing' }]);
  }
});

test('five crashes in a row without an answer: crash-loop', async () => {
  const statuses = [];
  const core = new Sidecar({ exe: FAKE, dataDir: DATA, onEvent() {}, onStatus: (s) => statuses.push(s), backoffBase: 1, spawnImpl: failing('exit') }).start();
  await until(() => statuses.some((s) => s.detail === 'crash-loop'));
  core.stop();
  assert.deepEqual(statuses[0], { state: 'down', detail: 'crashed' });
  assert.deepEqual(statuses.at(-1), { state: 'down', detail: 'crash-loop' });
});

test('retryNow: from down, start again at once with a fresh backoff', async () => {
  const statuses = [];
  const core = new Sidecar({ exe: FAKE, dataDir: DATA, onEvent() {}, onStatus: (s) => statuses.push(s), backoffBase: 60_000, spawnImpl: failing('exit') }).start();
  await until(() => statuses.some((s) => s.state === 'down'));
  assert.equal(core.retryNow(), true);
  assert.deepEqual(statuses.at(-1), { state: 'starting' });
  await until(() => statuses.at(-1).state === 'down');
  core.stop();
});

test('the log file is capped, rotates to .1, and keeps one line per entry', () => {
  const { createLog } = require('../src/logfile');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mvs-logs-'));
  const log = createLog(dir, { maxBytes: 300 });
  for (let i = 0; i < 20; i++) log(`line ${i}\nwith a break`);
  const text = fs.readFileSync(path.join(dir, 'mvs.log'), 'utf8');
  assert.ok(fs.statSync(path.join(dir, 'mvs.log')).size <= 300);
  assert.ok(fs.existsSync(path.join(dir, 'mvs.log.1')));
  assert.ok(!/with a break\n(?!\d)/.test(text.replace(/\n$/, '')) || text.split('\n').every((l) => !l || /^\d{4}-/.test(l)));
});

test('retryNow also replaces a running core (up but failing, QA-20)', async () => {
  const statuses = [];
  const { core } = start({ onStatus: (s) => statuses.push(s) });
  await core.request('getIdentity', {});
  const first = core.child;
  assert.equal(core.retryNow(), true);
  assert.notEqual(core.child, first);
  assert.deepEqual(statuses.at(-1), { state: 'starting' });
  assert.deepEqual(await core.request('getIdentity', {}), { ok: {} });
  assert.deepEqual(statuses.at(-1), { state: 'up' });
  core.stop();
});

test('the log file never holds addresses, peer IDs or 64-hex IDs', () => {
  const { redact } = require('../src/logfile');
  const peer = '12D3KooW' + 'A'.repeat(44);
  const line = `dial /dns4/relay.example.org/tcp/443/wss/p2p/${peer} from 192.0.2.7 and 2001:db8::1:2:3 for mvs://join/${'a'.repeat(64)}/${'b'.repeat(64)}`;
  const out = redact(line);
  for (const leak of ['relay.example.org', peer, '192.0.2.7', '2001:db8', 'a'.repeat(64)]) assert.ok(!out.includes(leak), leak);
  assert.match(out, /\/dns4\/<host>/);
  assert.match(out, /<peer>/);
  assert.match(out, /<ip>/);
  assert.match(out, /<id>/);
});

test('the configured relay host is hidden even as a bare name, any case', () => {
  const { createLog } = require('../src/logfile');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mvs-logs-'));
  const log = createLog(dir);
  log.hide('relay.example.org');
  log('connect to relay.example.org:443 failed; RELAY.Example.ORG unreachable');
  const text = fs.readFileSync(path.join(dir, 'mvs.log'), 'utf8');
  assert.ok(!/relay\.example\.org/i.test(text), text);
  assert.match(text, /<host>:443 failed; <host> unreachable/);
});
