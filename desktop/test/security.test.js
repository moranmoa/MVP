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
const micCheck = (over = {}) => ({ mediaType: 'audio', requestingUrl: PAGE, securityOrigin: 'app://mvs/', isMainFrame: true, ...over });

test('the microphone is granted to the internal origin', () => {
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq()), true);
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ securityOrigin: undefined })), true);
  assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', micCheck()), true);
  // Electron may run a check with no webContents.
  assert.equal(sec.allowPermissionCheck(undefined, 'media', 'app://mvs', micCheck()), true);
});

test('the microphone is denied to any other origin', () => {
  for (const other of ['https://evil.example/', 'http://localhost:5173/', 'app://evil/index.html', 'file:///C:/x.html', 'mvs://join/x', 'devtools://devtools/', 'data:text/html,x', '', 'not a url']) {
    // Request: the page, the requesting URL or the origin is somewhere else.
    assert.equal(sec.allowPermissionRequest(other, 'media', micReq()), false, 'page ' + other);
    assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ requestingUrl: other })), false, 'url ' + other);
    assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ securityOrigin: other })), false, 'origin ' + other);
    // Check: the same, plus the requesting origin.
    assert.equal(sec.allowPermissionCheck(other, 'media', 'app://mvs', micCheck()), false, 'check page ' + other);
    assert.equal(sec.allowPermissionCheck(PAGE, 'media', other, micCheck()), false, 'check origin ' + other);
    assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', micCheck({ securityOrigin: other })), false, 'check sec ' + other);
    assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', micCheck({ requestingUrl: other })), false, 'check url ' + other);
  }
  // A missing requesting URL or origin is not ours either.
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ requestingUrl: undefined })), false);
  assert.equal(sec.allowPermissionCheck(PAGE, 'media', undefined, micCheck()), false);
  // Subframes, even ours, never get it.
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ isMainFrame: false })), false);
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ isMainFrame: undefined })), false);
  assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', micCheck({ isMainFrame: false })), false);
});

test('the camera is denied, also next to the microphone', () => {
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ mediaTypes: ['video'] })), false);
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ mediaTypes: ['audio', 'video'] })), false);
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ mediaTypes: ['video', 'audio'] })), false);
  assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', micCheck({ mediaType: 'video' })), false);
  // An empty, missing or odd media list is not "audio only".
  for (const mediaTypes of [[], undefined, null, 'audio', ['audio', 'unknown'], [{}]]) {
    assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ mediaTypes })), false, JSON.stringify(mediaTypes));
  }
  for (const mediaType of ['unknown', undefined, '', 'AUDIO']) {
    assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', micCheck({ mediaType })), false, String(mediaType));
  }
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', undefined), false);
  assert.equal(sec.allowPermissionCheck(PAGE, 'media', 'app://mvs', undefined), false);
});

test('WebRTC uses only the default route interface', () => {
  assert.equal(sec.WEBRTC_IP_POLICY, 'default_public_and_private_interfaces');
});

test('screen capture is denied', () => {
  for (const perm of ['display-capture', 'screen', 'desktop', 'window-management', 'window-placement']) {
    assert.equal(sec.allowPermissionRequest(PAGE, perm, micReq()), false, perm);
    assert.equal(sec.allowPermissionCheck(PAGE, perm, 'app://mvs', micCheck()), false, perm);
  }
  // Desktop capture surfaces as 'media' with a video type; also refused.
  assert.equal(sec.allowPermissionRequest(PAGE, 'media', micReq({ mediaTypes: ['video'] })), false);
});

test('unknown and other permissions are denied, even for our own page', () => {
  for (const perm of ['geolocation', 'notifications', 'clipboard-read', 'clipboard-sanitized-write', 'openExternal', 'fullscreen', 'pointerLock', 'midi', 'midiSysex', 'hid', 'serial', 'usb', 'idle-detection', 'speaker-selection', 'storage-access', 'mediaKeySystem', 'unknown', 'microphone', 'audioCapture', 'Media', '', undefined]) {
    assert.equal(sec.allowPermissionRequest(PAGE, perm, micReq()), false, String(perm));
    assert.equal(sec.allowPermissionCheck(PAGE, perm, 'app://mvs', micCheck()), false, String(perm));
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
