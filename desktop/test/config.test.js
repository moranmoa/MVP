// The relay setting (src/config.js) and how it reaches mvs-core's arguments.
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { EventEmitter } = require('node:events');
const { relayFor, bootstrapFor, validRelay, logHost, MAX_RELAY, MAX_BOOTSTRAP, stunFor, validStun, stunHost, MAX_STUN } = require('../src/config');
const { createLog } = require('../src/logfile');
const { Sidecar } = require('../src/sidecar');

const ID = '12D3KooW' + 'A'.repeat(44);
const RELAY = `/dns4/relay.example.org/tcp/443/wss/p2p/${ID}`;

function resources(content) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mvs-res-'));
  if (content !== undefined) fs.writeFileSync(path.join(dir, 'mvs-config.json'), content);
  return dir;
}

test('valid relay addresses', () => {
  assert.equal(validRelay(RELAY), true);
  assert.equal(validRelay(`/ip4/192.0.2.1/udp/4001/quic-v1/p2p/${ID}`), true);
});

test('invalid relay values are refused', () => {
  for (const bad of [
    // #114: the peer ID must be pinned
    '/dns4/relay.example.org/tcp/443/wss',
    `/dns4/relay.example.org/tcp/443/wss/p2p/${ID.slice(0, 40)}`,
    `/dns4/relay.example.org/tcp/443/wss/p2p/${ID}0OIl`,
    `/dns4/relay.example.org/tcp/443/wss/p2p/${ID}/extra`,
    '', '/', 'relay.example.org:4001', ` ${RELAY}`, `${RELAY} --inspect`, `${RELAY}\n`, '/' + 'a'.repeat(MAX_RELAY), 42, null, {}, ['/x']]) {
    assert.equal(validRelay(bad), false, String(bad).slice(0, 40));
  }
});

test('packaged: the relay comes from resources/mvs-config.json', () => {
  assert.equal(relayFor({ packaged: true, resourcesPath: resources(JSON.stringify({ relay: RELAY })) }), RELAY);
});

test('packaged: no file, {} or an empty relay means no relay, silently', () => {
  const warnings = [];
  const warn = (m) => warnings.push(m);
  assert.equal(relayFor({ packaged: true, resourcesPath: resources(), warn }), null);
  assert.equal(relayFor({ packaged: true, resourcesPath: resources('{}'), warn }), null);
  assert.equal(relayFor({ packaged: true, resourcesPath: resources('{"relay":""}'), warn }), null);
  assert.deepEqual(warnings, []);
});

test('packaged: broken JSON or a bad relay is ignored with a warning', () => {
  const warnings = [];
  const warn = (m) => warnings.push(m);
  assert.equal(relayFor({ packaged: true, resourcesPath: resources('{relay'), warn }), null);
  assert.equal(relayFor({ packaged: true, resourcesPath: resources('{"relay":"tcp://evil --x"}'), warn }), null);
  assert.equal(relayFor({ packaged: true, resourcesPath: resources('{"relay":["/x"]}'), warn }), null);
  assert.equal(warnings.length, 3);
});

test('packaged: MVS_RELAY in the environment is ignored', () => {
  assert.equal(relayFor({ packaged: true, resourcesPath: resources(), env: { MVS_RELAY: RELAY } }), null);
});

test('checkout: the relay comes from MVS_RELAY only', () => {
  const dir = resources(JSON.stringify({ relay: '/ignored' }));
  assert.equal(relayFor({ packaged: false, resourcesPath: dir, env: { MVS_RELAY: RELAY } }), RELAY);
  assert.equal(relayFor({ packaged: false, resourcesPath: dir, env: {} }), null);
  assert.equal(relayFor({ packaged: false, env: { MVS_RELAY: 'nope' }, warn() {} }), null);
});

function spawnedArgs(options) {
  let args;
  const spawnImpl = (_exe, a) => {
    args = a;
    const child = new EventEmitter();
    child.stdout = new EventEmitter();
    child.stderr = new EventEmitter();
    child.stdin = { write() {}, end() {} };
    child.kill = () => {};
    return child;
  };
  new Sidecar({ exe: path.resolve('/opt/mvs-core'), dataDir: path.resolve('/data'), onEvent() {}, spawnImpl, ...options }).start();
  return args;
}

test('mvs-core gets --relay only when there is one', () => {
  const data = path.resolve('/data');
  assert.deepEqual(spawnedArgs({}), ['--data-dir', data]);
  assert.deepEqual(spawnedArgs({ relay: null }), ['--data-dir', data]);
  assert.deepEqual(spawnedArgs({ relay: RELAY }), ['--data-dir', data, '--relay', RELAY]);
});

test('extra switches are appended and must be strings', () => {
  const data = path.resolve('/data');
  assert.deepEqual(spawnedArgs({ relay: RELAY, extraArgs: ['--update-root', '/u'] }), ['--data-dir', data, '--relay', RELAY, '--update-root', '/u']);
  assert.throws(() => spawnedArgs({ extraArgs: [42] }), /strings/);
  assert.throws(() => spawnedArgs({ extraArgs: '--x' }), /strings/);
});

const TCP = `/dns4/relay-direct.example.org/tcp/4001/p2p/${ID}`;

test('packaged: bootstrap comes from resources/mvs-config.json, checked like the relay', () => {
  const warnings = [];
  const warn = (m) => warnings.push(m);
  assert.deepEqual(bootstrapFor({ packaged: true, resourcesPath: resources(JSON.stringify({ relay: RELAY, bootstrap: [TCP] })) }), [TCP]);
  assert.deepEqual(bootstrapFor({ packaged: true, resourcesPath: resources(JSON.stringify({ relay: RELAY })), warn }), []);
  assert.deepEqual(bootstrapFor({ packaged: true, resourcesPath: resources(), warn }), []);
  assert.deepEqual(warnings, [], 'absent bootstrap is not a warning');
  const mixed = { bootstrap: [TCP, '/dns4/x/tcp/4001', `${TCP} --inspect`, 42] };
  assert.deepEqual(bootstrapFor({ packaged: true, resourcesPath: resources(JSON.stringify(mixed)), warn }), [TCP]);
  assert.deepEqual(bootstrapFor({ packaged: true, resourcesPath: resources('{"bootstrap":"/x"}'), warn }), []);
  const many = { bootstrap: Array(MAX_BOOTSTRAP + 2).fill(TCP) };
  assert.equal(bootstrapFor({ packaged: true, resourcesPath: resources(JSON.stringify(many)), warn }).length, MAX_BOOTSTRAP);
  assert.equal(warnings.length, 3);
});

test('checkout: bootstrap comes from MVS_BOOTSTRAP only', () => {
  const dir = resources(JSON.stringify({ bootstrap: [TCP] }));
  assert.deepEqual(bootstrapFor({ packaged: false, resourcesPath: dir, env: {} }), []);
  assert.deepEqual(bootstrapFor({ packaged: false, env: { MVS_BOOTSTRAP: `${TCP},${TCP}` } }), [TCP, TCP]);
});

test('mvs-core gets --bootstrap after --relay, and only with a relay', () => {
  const data = path.resolve('/data');
  assert.deepEqual(spawnedArgs({ relay: RELAY, bootstrap: [TCP] }), ['--data-dir', data, '--relay', RELAY, '--bootstrap', TCP]);
  assert.deepEqual(spawnedArgs({ relay: null, bootstrap: [TCP] }), ['--data-dir', data]);
  assert.throws(() => spawnedArgs({ bootstrap: [42] }), /strings/);
  assert.throws(() => spawnedArgs({ bootstrap: TCP }), /strings/);
});

// STUN for voice (#222): a plain stun:/stuns: URL, the same rule as the UI's iceServersFor.
test('valid STUN URLs', () => {
  for (const ok of ['stun:stun.example.org:3478', 'stuns:stun.example.org:5349', 'STUN:stun.example.org', 'stun:198.51.100.7:3478', 'stun:[2001:db8::7]:3478', '  stun:stun.example.org:3478  ']) {
    assert.equal(validStun(ok), true, ok);
  }
});

test('anything but a plain STUN URL is refused', () => {
  for (const bad of ['turn:turn.example.org:3478', 'turns:turn.example.org', 'stun:user:pass@stun.example.org', 'stun:stun.example.org/path', 'stun:stun.example.org?transport=udp', 'stun:stun.example.org#x', 'stun:stun example.org', 'stun:', 'https://stun.example.org', '//stun.example.org', 'stun.example.org:3478', 'stun:' + 'a'.repeat(MAX_STUN), '', 7, null, ['stun:stun.example.org'], {}]) {
    assert.equal(validStun(bad), false, String(bad));
  }
});

test('packaged: STUN comes from mvs-config.json; MVS_STUN is ignored', () => {
  const warns = [];
  const warn = (m) => warns.push(m);
  const env = { MVS_STUN: 'stun:env.example.org:3478' };
  assert.equal(stunFor({ packaged: true, resourcesPath: resources(JSON.stringify({ relay: RELAY, stun: ' stun:stun.example.org:3478 ' })), env, warn }), 'stun:stun.example.org:3478');
  assert.equal(stunFor({ packaged: true, resourcesPath: resources(), env, warn }), null, 'no file');
  assert.equal(stunFor({ packaged: true, resourcesPath: resources(JSON.stringify({ relay: RELAY })), env, warn }), null, 'no stun key');
  assert.equal(stunFor({ packaged: true, resourcesPath: resources('{nope'), env, warn }), null, 'broken JSON');
  assert.deepEqual(warns, []);
  assert.equal(stunFor({ packaged: true, resourcesPath: resources(JSON.stringify({ stun: 'turn:t.example.org:3478' })), env, warn }), null);
  assert.equal(stunFor({ packaged: true, resourcesPath: resources(JSON.stringify({ stun: ['stun:a.example.org'] })), env, warn }), null);
  assert.equal(warns.length, 2);
  // The warning never names the value.
  assert.ok(warns.every((w) => !w.includes('example')), warns.join());
});

test('checkout: STUN comes from MVS_STUN only', () => {
  const warns = [];
  const warn = (m) => warns.push(m);
  assert.equal(stunFor({ packaged: false, env: { MVS_STUN: 'stun:stun.example.org:3478' }, warn }), 'stun:stun.example.org:3478');
  assert.equal(stunFor({ packaged: false, env: {}, warn }), null);
  assert.equal(stunFor({ packaged: false, env: { MVS_STUN: '' }, warn }), null);
  assert.equal(stunFor({ packaged: false, env: { MVS_STUN: 'stun:u:p@stun.example.org' }, warn }), null);
  assert.equal(warns.length, 1);
});

test('the STUN host is found for the file log to hide', () => {
  assert.equal(stunHost('stun:stun.example.org:3478'), 'stun.example.org');
  assert.equal(stunHost('stuns:stun.example.org'), 'stun.example.org');
  assert.equal(stunHost('stun:198.51.100.7:3478'), '198.51.100.7');
  assert.equal(stunHost('stun:[2001:db8::7]:3478'), '2001:db8::7');
  assert.equal(stunHost(null), null);
});

test('relay and bootstrap hosts, DNS or IP, never reach the file log', () => {
  const peer = '12D3KooWBVzkbuxE3TA4LU7zEeMBM87G4Ns1KQoPdAc4MR8R3GTc';
  assert.equal(logHost(`/dns4/relay.example.org/tcp/443/wss/p2p/${peer}`), 'relay.example.org');
  assert.equal(logHost(`/ip4/198.51.100.7/tcp/4001/p2p/${peer}`), '198.51.100.7');
  assert.equal(logHost(`/ip6/2001:db8::7/udp/4001/quic-v1/p2p/${peer}`), '2001:db8::7');
  assert.equal(logHost(`/p2p/${peer}`), null);
  assert.equal(logHost(null), null);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mvs-loghost-'));
  try {
    const log = createLog(dir);
    for (const addr of [`/ip4/198.51.100.7/tcp/4001/p2p/${peer}`, `/ip6/2001:db8::7/udp/4001/quic-v1/p2p/${peer}`]) {
      log.hide(logHost(addr));
    }
    // Glued to other text, past the generic IP redaction's word boundaries.
    log('core: dial failed to v198.51.100.7x and [2001:db8::7]:4001');
    const text = fs.readFileSync(path.join(dir, 'mvs.log'), 'utf8');
    assert.ok(!text.includes('198.51.100.7') && !text.includes('2001:db8::7'), text);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
