// Unit tests for the shell's security policy (src/security.js). Plain Node:
//   docker compose run --rm test npm test
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const sec = require('../src/security');

const ROOT = path.resolve('/srv/ui');
const META = `<head><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'"><title>x</title></head>`;

test('renderer preferences are locked down and frozen', () => {
  const p = sec.WEB_PREFERENCES;
  assert.equal(p.sandbox, true);
  assert.equal(p.contextIsolation, true);
  assert.equal(p.nodeIntegration, false);
  assert.equal(p.nodeIntegrationInWorker, false);
  assert.equal(p.nodeIntegrationInSubFrames, false);
  assert.equal(p.webSecurity, true);
  assert.equal(p.allowRunningInsecureContent, false);
  assert.equal(p.webviewTag, false);
  assert.equal(p.experimentalFeatures, false);
  assert.equal(p.preload, undefined, 'main.js adds the one preload; nothing else may');
  assert.ok(Object.isFrozen(p));
});

test('no permission is granted', () => {
  for (const perm of ['media', 'geolocation', 'notifications', 'clipboard-read', 'openExternal', 'fullscreen', 'hid', 'serial']) {
    assert.equal(sec.ALLOWED_PERMISSIONS.has(perm), false, perm);
  }
});

// Voice v1 (piece C): the microphone for app://mvs only; nothing else.
const PAGE = 'app://mvs/index.html';
const micReq = (over = {}) => ({ mediaTypes: ['audio'], requestingUrl: PAGE, securityOrigin: 'app://mvs/', isMainFrame: true, ...over });
// The rules below are checked during a call (the strictest case for a
// denial); V-C2's tests check the out-of-call side.
const allowReq = (page, perm, details, inCall = true) => sec.allowPermissionRequest(page, perm, details, inCall);
const allowCheck = (page, perm, origin, details, inCall = true) => sec.allowPermissionCheck(page, perm, origin, details, inCall);
const micCheck = (over = {}) => ({ mediaType: 'audio', requestingUrl: PAGE, securityOrigin: 'app://mvs/', isMainFrame: true, ...over });

test('the microphone is granted to the internal origin', () => {
  assert.equal(allowReq(PAGE, 'media', micReq()), true);
  assert.equal(allowReq(PAGE, 'media', micReq({ securityOrigin: undefined })), true);
  assert.equal(allowCheck(PAGE, 'media', 'app://mvs', micCheck()), true);
  // Electron may run a check with no webContents.
  assert.equal(allowCheck(undefined, 'media', 'app://mvs', micCheck()), true);
});

test('the microphone is denied to any other origin', () => {
  for (const other of ['https://evil.example/', 'http://localhost:5173/', 'app://evil/index.html', 'file:///C:/x.html', 'mvs://join/x', 'devtools://devtools/', 'data:text/html,x', '', 'not a url']) {
    // Request: the page, the requesting URL or the origin is somewhere else.
    assert.equal(allowReq(other, 'media', micReq()), false, 'page ' + other);
    assert.equal(allowReq(PAGE, 'media', micReq({ requestingUrl: other })), false, 'url ' + other);
    assert.equal(allowReq(PAGE, 'media', micReq({ securityOrigin: other })), false, 'origin ' + other);
    // Check: the same, plus the requesting origin.
    assert.equal(allowCheck(other, 'media', 'app://mvs', micCheck()), false, 'check page ' + other);
    assert.equal(allowCheck(PAGE, 'media', other, micCheck()), false, 'check origin ' + other);
    assert.equal(allowCheck(PAGE, 'media', 'app://mvs', micCheck({ securityOrigin: other })), false, 'check sec ' + other);
    assert.equal(allowCheck(PAGE, 'media', 'app://mvs', micCheck({ requestingUrl: other })), false, 'check url ' + other);
  }
  // A missing requesting URL or origin is not ours either.
  assert.equal(allowReq(PAGE, 'media', micReq({ requestingUrl: undefined })), false);
  assert.equal(allowCheck(PAGE, 'media', undefined, micCheck()), false);
  // Subframes, even ours, never get it.
  assert.equal(allowReq(PAGE, 'media', micReq({ isMainFrame: false })), false);
  assert.equal(allowReq(PAGE, 'media', micReq({ isMainFrame: undefined })), false);
  assert.equal(allowCheck(PAGE, 'media', 'app://mvs', micCheck({ isMainFrame: false })), false);
});

test('the camera is denied, also next to the microphone', () => {
  assert.equal(allowReq(PAGE, 'media', micReq({ mediaTypes: ['video'] })), false);
  assert.equal(allowReq(PAGE, 'media', micReq({ mediaTypes: ['audio', 'video'] })), false);
  assert.equal(allowReq(PAGE, 'media', micReq({ mediaTypes: ['video', 'audio'] })), false);
  assert.equal(allowCheck(PAGE, 'media', 'app://mvs', micCheck({ mediaType: 'video' })), false);
  // An empty, missing or odd media list is not "audio only".
  for (const mediaTypes of [[], undefined, null, 'audio', ['audio', 'unknown'], [{}]]) {
    assert.equal(allowReq(PAGE, 'media', micReq({ mediaTypes })), false, JSON.stringify(mediaTypes));
  }
  for (const mediaType of ['unknown', undefined, '', 'AUDIO']) {
    assert.equal(allowCheck(PAGE, 'media', 'app://mvs', micCheck({ mediaType })), false, String(mediaType));
  }
  assert.equal(allowReq(PAGE, 'media', undefined), false);
  assert.equal(allowCheck(PAGE, 'media', 'app://mvs', undefined), false);
});

test('WebRTC uses only the default route interface', () => {
  assert.equal(sec.WEBRTC_IP_POLICY, 'default_public_and_private_interfaces');
});

// V-C2: the microphone only during a call.
test('the microphone is denied outside a call', () => {
  for (const inCall of [false, undefined, null, 1, 'true', {}]) {
    assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq(), inCall), false, String(inCall));
    assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', micCheck(), inCall), false, String(inCall));
  }
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq()), false, 'no flag given');
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq(), true), true);
  assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', micCheck(), true), true);
});

const CALL = { channelId: 'c'.repeat(64), participants: [{ identity: 'a'.repeat(64), muted: false }], full: false };
const LEFT = { ...CALL, participants: [] };
const joined = (gate) => {
  const t = gate.beforeCommand('joinCall');
  gate.afterCommand('joinCall', t, { ok: CALL });
};

test('call gate: shut at start, open after a successful joinCall', () => {
  const gate = sec.createCallGate();
  assert.equal(gate.inCall, false);
  joined(gate);
  assert.equal(gate.inCall, true);
  // Other commands and events leave it as it is.
  gate.afterCommand('setMuted', gate.beforeCommand('setMuted'), { ok: null });
  gate.onEvent('call.state', CALL);
  gate.onEvent('call.signal', { from: 'x' });
  gate.onCoreStatus({ state: 'up' });
  assert.equal(gate.inCall, true);
});

test('call gate: a failed or empty joinCall never opens it, and shuts an open one', () => {
  for (const res of [{ error: { code: 'call-full' } }, { error: { code: 'not-implemented' } }, { ok: LEFT }, { ok: null }, { ok: {} }, { ok: { participants: 'x' } }, null, undefined]) {
    const gate = sec.createCallGate();
    gate.afterCommand('joinCall', gate.beforeCommand('joinCall'), res);
    assert.equal(gate.inCall, false, JSON.stringify(res));
    joined(gate);
    // joinCall leaves the current call first, so a failed one leaves us out.
    gate.afterCommand('joinCall', gate.beforeCommand('joinCall'), res);
    assert.equal(gate.inCall, false, 'after an open gate: ' + JSON.stringify(res));
  }
});

test('call gate: leave, end, core down and quit shut it', () => {
  const cases = {
    leaveCall: (g) => g.beforeCommand('leaveCall'),
    'call.state with no participants': (g) => g.onEvent('call.state', LEFT),
    'call.state with no payload': (g) => g.onEvent('call.state', null),
    'core down': (g) => g.onCoreStatus({ state: 'down', detail: 'crash' }),
    'core restarting': (g) => g.onCoreStatus({ state: 'starting' }),
    'core status missing': (g) => g.onCoreStatus(undefined),
    quit: (g) => g.end(),
  };
  for (const [label, act] of Object.entries(cases)) {
    const gate = sec.createCallGate();
    joined(gate);
    act(gate);
    assert.equal(gate.inCall, false, label);
  }
});

test('call gate: a late joinCall answer after a leave, a core restart or quit stays shut', () => {
  for (const [label, act] of [
    ['leaveCall', (g) => g.beforeCommand('leaveCall')],
    ['core restart', (g) => g.onCoreStatus({ state: 'starting' })],
    ['quit', (g) => g.end()],
  ]) {
    const gate = sec.createCallGate();
    const t = gate.beforeCommand('joinCall');
    act(gate);
    gate.afterCommand('joinCall', t, { ok: CALL });
    assert.equal(gate.inCall, false, label);
  }
});

test('call gate: switching calls (the old call.state [] comes first) ends in the new call', () => {
  const gate = sec.createCallGate();
  joined(gate);
  const t = gate.beforeCommand('joinCall');
  // The core leaves the old call first; its event comes before the answer.
  gate.onEvent('call.state', LEFT);
  assert.equal(gate.inCall, false);
  gate.onEvent('call.state', CALL);
  gate.afterCommand('joinCall', t, { ok: CALL });
  assert.equal(gate.inCall, true);
});

test('screen capture is denied', () => {
  for (const perm of ['display-capture', 'screen', 'desktop', 'window-management', 'window-placement']) {
    assert.equal(allowReq(PAGE, perm, micReq()), false, perm);
    assert.equal(allowCheck(PAGE, perm, 'app://mvs', micCheck()), false, perm);
  }
  // Desktop capture surfaces as 'media' with a video type; also refused.
  assert.equal(allowReq(PAGE, 'media', micReq({ mediaTypes: ['video'] })), false);
});

test('unknown and other permissions are denied, even for our own page', () => {
  for (const perm of ['geolocation', 'notifications', 'clipboard-read', 'clipboard-sanitized-write', 'openExternal', 'fullscreen', 'pointerLock', 'midi', 'midiSysex', 'hid', 'serial', 'usb', 'idle-detection', 'speaker-selection', 'storage-access', 'mediaKeySystem', 'unknown', 'microphone', 'audioCapture', 'Media', '', undefined]) {
    assert.equal(allowReq(PAGE, perm, micReq()), false, String(perm));
    assert.equal(allowCheck(PAGE, perm, 'app://mvs', micCheck()), false, String(perm));
  }
});

test('CSP comes from the built page and gains frame-ancestors', () => {
  assert.equal(sec.cspFromIndexHtml(META), "default-src 'none'; script-src 'self'; frame-ancestors 'none'");
});

test('CSP written by Vite (entity-encoded quotes, content first) is read correctly', () => {
  const html = `<meta content="default-src &#39;none&#39;; img-src &#39;self&#39; data:" http-equiv="Content-Security-Policy">`;
  assert.equal(sec.cspFromIndexHtml(html), "default-src 'none'; img-src 'self' data:; frame-ancestors 'none'");
});

test('no CSP meta tag: refuse to start', () => {
  assert.throws(() => sec.cspFromIndexHtml('<head><title>x</title></head>'), /no Content-Security-Policy/);
});

test('a loose CSP is refused', () => {
  for (const bad of ["script-src 'self' 'unsafe-eval'", "style-src 'unsafe-inline'", 'script-src https://cdn.example', 'default-src *']) {
    const html = `<meta http-equiv="Content-Security-Policy" content="${bad}">`;
    assert.throws(() => sec.cspFromIndexHtml(html), /loose CSP/, bad);
  }
});

test('response headers carry the policy and nosniff', () => {
  const h = sec.responseHeaders("default-src 'none'; frame-ancestors 'none'", 'text/html');
  assert.equal(h['content-security-policy'], "default-src 'none'; frame-ancestors 'none'");
  assert.equal(h['x-content-type-options'], 'nosniff');
  assert.equal(h['content-type'], 'text/html');
});

test('app:// paths resolve inside the UI folder only', () => {
  assert.equal(sec.resolveAppPath(ROOT, 'app://mvs/'), path.join(ROOT, 'index.html'));
  assert.equal(sec.resolveAppPath(ROOT, 'app://mvs/index.html?demo=1'), path.join(ROOT, 'index.html'));
  assert.equal(sec.resolveAppPath(ROOT, 'app://mvs/assets/a.js'), path.join(ROOT, 'assets', 'a.js'));
});

test('a literal ../ is collapsed by the URL parser and stays inside', () => {
  assert.equal(sec.resolveAppPath(ROOT, 'app://mvs/../etc/passwd'), path.join(ROOT, 'etc', 'passwd'));
});

test('hostile app:// URLs resolve to nothing', () => {
  for (const url of [
    'app://mvs/%2e%2e/%2e%2e/etc/passwd',
    'app://mvs/assets/%2e%2e%2f%2e%2e%2fsecret',
    'app://mvs/..%5c..%5cwindows%5cwin.ini',
    'app://mvs/a%00.js',
    'app://mvs/%E0%A4%A',
    'app://other/index.html',
    'file:///etc/passwd',
    'https://mvs/index.html',
    'not a url',
  ]) {
    const r = sec.resolveAppPath(ROOT, url);
    assert.ok(r === null || r.startsWith(ROOT + path.sep), `${url} -> ${r}`);
    if (/\.\.|%00|%5c|other|file:|https:|not a/.test(url)) assert.equal(r, null, url);
  }
});

test('only app://mvs is internal', () => {
  assert.equal(sec.isInternalUrl('app://mvs/index.html'), true);
  for (const url of ['app://mvs.evil/', 'app://evil/', 'https://mvs/', 'file:///etc', 'javascript:alert(1)', 'data:text/html,x', '']) {
    assert.equal(sec.isInternalUrl(url), false, url);
  }
});

test('only plain https links may be offered to the OS browser', () => {
  assert.equal(sec.isExternalWebUrl('https://example.org/a?b=c'), true);
  for (const url of ['http://example.org/', 'file:///C:/Windows/System32/calc.exe', 'javascript:alert(1)', 'smb://host/share', 'ms-settings:', 'https://user:pw@example.org/', 'app://mvs/', 'data:,x', '']) {
    assert.equal(sec.isExternalWebUrl(url), false, url);
  }
});
