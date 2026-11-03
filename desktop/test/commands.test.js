// The allow-list (src/commands.js) against the UI's contract and hostile requests.
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { COMMANDS, MAX_REQUEST_BYTES, toRequest } = require('../src/commands');

/** Method names and parameter names of `interface CoreCommands` in ui/src/core/api.ts. */
function contract() {
  const src = fs.readFileSync(path.join(__dirname, '..', '..', 'ui', 'src', 'core', 'api.ts'), 'utf8');
  const body = /export interface CoreCommands \{([\s\S]*?)\n\}/.exec(src)[1].replace(/\/\*[\s\S]*?\*\/|\/\/.*$/gm, '');
  const out = {};
  for (const m of body.matchAll(/^\s*(\w+)\(([^)]*)\)\s*:/gm)) {
    // Split on top-level commas only: `Record<number, string>` is one parameter.
    const params = [];
    let depth = 0;
    let cur = '';
    for (const ch of m[2]) {
      if ('<{[('.includes(ch)) depth++;
      if ('>}])'.includes(ch)) depth--;
      if (ch === ',' && depth === 0) {
        params.push(cur);
        cur = '';
      } else cur += ch;
    }
    params.push(cur);
    out[m[1]] = params.map((p) => p.trim().split(/[?:]/)[0].trim()).filter(Boolean);
  }
  return out;
}

test('the allow-list is exactly CoreCommands, with the same parameter names', () => {
  assert.deepEqual(COMMANDS, contract());
});

test('commands outside the list are refused, including prototype names', () => {
  for (const cmd of ['grantRole', 'revokeRole', 'syncNow', 'constructor', '__proto__', 'toString', 'hasOwnProperty', '', 42, null]) {
    assert.deepEqual(toRequest(cmd, []), { error: 'not-allowed' }, String(cmd));
  }
});

test('positional args become the core\'s named args', () => {
  assert.deepEqual(toRequest('getMessages', ['c1', 0, 100]), { cmd: 'getMessages', args: { channelId: 'c1', start: 0, end: 100 } });
  assert.deepEqual(toRequest('sendMessage', ['c1', 'hi']), { cmd: 'sendMessage', args: { channelId: 'c1', body: 'hi' } });
  assert.deepEqual(toRequest('getIdentity', []), { cmd: 'getIdentity', args: {} });
});

test('malformed args are refused', () => {
  assert.deepEqual(toRequest('getIdentity', ['extra']), { error: 'bad-request' });
  assert.deepEqual(toRequest('listChannels', 's1'), { error: 'bad-request' });
  assert.deepEqual(toRequest('listChannels', { serverId: 's1' }), { error: 'bad-request' });
  const cyclic = {};
  cyclic.self = cyclic;
  assert.deepEqual(toRequest('updateProfile', [cyclic]), { error: 'bad-request' });
});

test('requests over 1 MiB are refused before they reach the core', () => {
  assert.deepEqual(toRequest('sendMessage', ['c1', 'x'.repeat(MAX_REQUEST_BYTES)]), { error: 'too-large' });
  assert.equal(toRequest('sendMessage', ['c1', 'x'.repeat(1000)]).cmd, 'sendMessage');
});
