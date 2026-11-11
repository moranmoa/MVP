// G-61: desktop notifications, shown by main with Electron's Notification.
// The page never gets the `notifications` permission (security.js still
// denies it); it asks main through one IPC call, `notify:show`, and main
// checks what it is given. A click brings the window up and tells the page
// which channel to open (`notify.click`).
'use strict';

const TITLE_MAX = 200;
const BODY_MAX = 300;
const ID_MAX = 128;
/** At most this many notifications a minute, whatever the page asks. */
const PER_MINUTE = 20;

// Control characters (and bidi overrides, which could reorder what the OS
// shows) are dropped; isolates and marks stay.
const STRIP = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f‪-‮]/g;

function text(v, max) {
  if (typeof v !== 'string') return null;
  const s = v.replace(STRIP, '').replace(/\s+/g, ' ').trim();
  if (!s) return null;
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

function id(v) {
  return typeof v === 'string' && v.length > 0 && v.length <= ID_MAX && /^[\w.:-]+$/.test(v) ? v : null;
}

/** `{ title, body, channelId, messageId? }` as the page sent it, or null if anything is off. */
function cleanNotice(n) {
  if (!n || typeof n !== 'object' || Array.isArray(n)) return null;
  const title = text(n.title, TITLE_MAX);
  const body = text(n.body, BODY_MAX);
  const channelId = id(n.channelId);
  if (!title || !body || !channelId) return null;
  const messageId = n.messageId === undefined ? null : id(n.messageId);
  if (n.messageId !== undefined && !messageId) return null;
  return { title, body, channelId, messageId };
}

/**
 * Shows notices; a new one for the same channel replaces the last one (the
 * page groups a channel's messages within 10 s into one, G-61).
 * `Notification` is Electron's class (a fake in tests); `onClick` gets
 * `{ channelId, messageId }`.
 */
function createNotifier({ Notification, onClick, now = Date.now }) {
  const shown = new Map();
  let windowStart = 0;
  let count = 0;
  return {
    show(raw) {
      const n = cleanNotice(raw);
      if (!n || !Notification.isSupported()) return false;
      if (now() - windowStart >= 60_000) {
        windowStart = now();
        count = 0;
      }
      if (count >= PER_MINUTE) return false;
      count++;
      shown.get(n.channelId)?.close();
      // No icon or actions from the page; silent: sound is G-63's switch.
      const note = new Notification({ title: n.title, body: n.body, silent: true });
      note.on('click', () => onClick({ channelId: n.channelId, messageId: n.messageId }));
      note.on('close', () => shown.get(n.channelId) === note && shown.delete(n.channelId));
      shown.set(n.channelId, note);
      note.show();
      return true;
    },
  };
}

module.exports = { cleanNotice, createNotifier, PER_MINUTE };
