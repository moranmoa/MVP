// mvs:// parsing (src/deeplink.js): the exact invite format, nothing else.
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { parseInviteLink, inviteFromArgv } = require('../src/deeplink');

const S = 'a'.repeat(64);
const I = '0123456789abcdef'.repeat(4);
const GOOD = `mvs://join/${S}/${I}`;

test('a well-formed invite link passes, canonicalised', () => {
  assert.equal(parseInviteLink(GOOD), GOOD);
  assert.equal(parseInviteLink(`${GOOD}/`), GOOD, 'trailing slash some OSes add');
  assert.equal(parseInviteLink(GOOD.toUpperCase()), GOOD);
  assert.equal(parseInviteLink(`  ${GOOD}\n`), GOOD);
});

test('anything else is refused', () => {
  for (const bad of [
    `mvs://join/${S}`,
    `mvs://join/${S}/${I}/extra`,
    `mvs://join/${S.slice(1)}/${I}`,
    `mvs://join/${S}/${I}0`,
    `mvs://join/${'g'.repeat(64)}/${I}`,
    `mvs://other/${S}/${I}`,
    `mvs://join/${S}/${I}?x=1`,
    `mvs://join/${S}/${I}#x`,
    `mvs://user@join/${S}/${I}`,
    `mvs:join/${S}/${I}`,
    `https://join/${S}/${I}`,
    `mvs://join/../../${S}/${I}`,
    `mvs://join/${S}/%2e%2e`,
    `mvs://join/${S}/${I}\u0000`,
    `mvs://join/${S}/${I} --inspect`,
    'x'.repeat(10_000),
    '',
    null,
    42,
  ]) {
    assert.equal(parseInviteLink(bad), null, String(bad).slice(0, 80));
  }
});

test('the link is found in a command line, and only an mvs:// argument counts', () => {
  assert.equal(inviteFromArgv(['C:\\Program Files\\MVS\\mvs.exe', '--allow-file-access', GOOD]), GOOD);
  assert.equal(inviteFromArgv(['mvs', `--x=${GOOD}`]), null);
  assert.equal(inviteFromArgv(['mvs', 'mvs://join/bad']), null);
  assert.equal(inviteFromArgv(['mvs']), null);
  assert.equal(inviteFromArgv('mvs://join'), null);
});
