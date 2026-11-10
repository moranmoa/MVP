'use strict';

const test = require('node:test');
const assert = require('node:assert');
const { redact } = require('../src/logfile');

test('IPv6 addresses are redacted, in every common form', () => {
  for (const [line, want] of [
    ['peer at fe80::1 now', 'peer at <ip> now'],
    ['dial 2001:db8::7 failed', 'dial <ip> failed'],
    ['bound ::1', 'bound <ip>'],
    ['mapped ::ffff:203.0.113.7', 'mapped <ip>'],
    ['/ip6/2001:db8:0:0:1::2/tcp/4001', '/ip6/<ip>/tcp/4001'],
    ['[fe80::1]:4001', '[<ip>]:4001'],
    ['zone fe80::1%eth0', 'zone <ip>%eth0'],
    ['full 2001:0db8:0000:0000:0000:ff00:0042:8329', 'full <ip>'],
    ['trailing fe80::', 'trailing <ip>'],
    // Next to a colon (security lead on #308): these leaked with the first fix.
    ['dial 2001:db8::7: timeout', 'dial <ip>: timeout'],
    ['addr:2001:db8::7', 'addr:<ip>'],
    ['peer=fe80::1:', 'peer=<ip>:'],
  ]) {
    assert.strictEqual(redact(line), want, line);
  }
});

test('Rust paths and clock times are left alone', () => {
  for (const line of [
    'mvs::join timed out',
    'sync::node: dial',
    'api::add_member done',
    'core::api::service ok',
    'sync::node::dial failed',
    'api::add',
    'at 12:30:45 restart',
  ]) {
    assert.strictEqual(redact(line), line);
  }
});

test('IPv4, ids and peer ids are still redacted', () => {
  assert.strictEqual(redact('from 203.0.113.7'), 'from <ip>');
  assert.strictEqual(redact(`id ${'a'.repeat(64)}`), 'id <id>');
  assert.strictEqual(redact('peer 12D3KooWAbCdEfGhJkLmNoPqRsTuVwXyZaBcDeFgHiJkLmNoPq'), 'peer <peer>');
});
