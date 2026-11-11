'use strict';

const test = require('node:test');
const assert = require('node:assert');
const { EventEmitter } = require('events');
const { cleanNotice, createNotifier, PER_MINUTE } = require('../src/notify');

function fakeNotification() {
  const made = [];
  class FakeNotification extends EventEmitter {
    constructor(opts) {
      super();
      this.opts = opts;
      this.shown = false;
      this.closed = false;
      made.push(this);
    }
    static isSupported() {
      return true;
    }
    show() {
      this.shown = true;
    }
    close() {
      this.closed = true;
      this.emit('close');
    }
  }
  return { FakeNotification, made };
}

const ok = { title: 'Noa · #general · Gamers', body: 'Anyone up tonight?', channelId: 'c-general', messageId: 'm-1' };

test('cleanNotice keeps plain text and checked ids only', () => {
  assert.deepStrictEqual(cleanNotice(ok), { ...ok });
  assert.deepStrictEqual(cleanNotice({ ...ok, messageId: undefined }), { ...ok, messageId: null });
  // Control and bidi-override characters go; whitespace collapses.
  assert.equal(cleanNotice({ ...ok, body: 'a\u0007‮ b\n\nc' }).body, 'a b c');
  assert.equal(cleanNotice({ ...ok, body: 'x'.repeat(1000) }).body.length, 300);
  for (const bad of [null, 'x', [], {}, { ...ok, title: '' }, { ...ok, body: 5 }, { ...ok, channelId: '../x' }, { ...ok, channelId: 'a'.repeat(200) }, { ...ok, messageId: 'bad id' }]) {
    assert.equal(cleanNotice(bad), null, JSON.stringify(bad));
  }
});

test('shows a silent notification with only the cleaned fields; a click reports the channel', () => {
  const { FakeNotification, made } = fakeNotification();
  const clicks = [];
  const n = createNotifier({ Notification: FakeNotification, onClick: (t) => clicks.push(t) });
  assert.equal(n.show({ ...ok, icon: 'file:///evil.png', actions: [{ text: 'x' }] }), true);
  assert.deepStrictEqual(made[0].opts, { title: ok.title, body: ok.body, silent: true });
  assert.equal(made[0].shown, true);
  made[0].emit('click');
  assert.deepStrictEqual(clicks, [{ channelId: 'c-general', messageId: 'm-1' }]);
  assert.equal(n.show({ title: 'x' }), false);
  assert.equal(made.length, 1);
});

test('a new notice for the same channel replaces the last one', () => {
  const { FakeNotification, made } = fakeNotification();
  const n = createNotifier({ Notification: FakeNotification, onClick: () => {} });
  n.show(ok);
  n.show({ ...ok, channelId: 'c-other' });
  n.show({ ...ok, body: '3 new messages in #general' });
  assert.deepStrictEqual(made.map((m) => m.closed), [true, false, false]);
});

test(`at most ${PER_MINUTE} a minute`, () => {
  const { FakeNotification } = fakeNotification();
  let t = 1_000_000;
  const n = createNotifier({ Notification: FakeNotification, onClick: () => {}, now: () => t });
  for (let i = 0; i < PER_MINUTE; i++) assert.equal(n.show({ ...ok, channelId: `c-${i}` }), true);
  assert.equal(n.show(ok), false);
  t += 60_000;
  assert.equal(n.show(ok), true);
});

test('nothing when the OS has no notifications', () => {
  const { FakeNotification } = fakeNotification();
  FakeNotification.isSupported = () => false;
  assert.equal(createNotifier({ Notification: FakeNotification, onClick: () => {} }).show(ok), false);
});
