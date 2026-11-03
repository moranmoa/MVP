// Launches the real app (Electron + ui/dist) and checks the security settings
// from the inside. Needs a display: run in Docker with
//   docker compose run --rm test
import test, { after, before } from 'node:test';
import assert from 'node:assert/strict';
import { _electron as electron } from 'playwright-core';

let app;
let page;

before(async () => {
  app = await electron.launch({ args: ['.'], env: { ...process.env, MVS_DEMO: '1' } });
  page = await app.firstWindow();
  await page.waitForFunction(() => document.querySelector('#root')?.children.length > 0, null, { timeout: 30_000 });
});

after(async () => {
  await app?.close();
});

test('the UI renders from app://mvs', async () => {
  assert.equal(await page.evaluate(() => location.origin), 'app://mvs');
});

test('window preferences as configured', async () => {
  const prefs = await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].webContents.getLastWebPreferences());
  assert.equal(prefs.sandbox, true);
  assert.equal(prefs.contextIsolation, true);
  assert.equal(prefs.nodeIntegration, false);
  assert.equal(prefs.webSecurity, true);
  assert.equal(prefs.webviewTag, false);
});

test('no Node or Electron in the page', async () => {
  const seen = await page.evaluate(() => [typeof require, typeof process, typeof module, typeof globalThis.electron]);
  assert.deepEqual(seen, ['undefined', 'undefined', 'undefined', 'undefined']);
});

test('the CSP header is sent and enforced', async () => {
  const csp = await page.evaluate(async () => (await fetch('/index.html')).headers.get('content-security-policy'));
  assert.match(csp, /default-src 'none'/);
  assert.match(csp, /frame-ancestors 'none'/);
  // (eval cannot be tested from here: DevTools-evaluated code is exempt.)
  const result = await page.evaluate(
    () =>
      new Promise((resolve) => {
        document.addEventListener('securitypolicyviolation', (e) => resolve({ ran: !!window.__inline, blocked: e.violatedDirective }), { once: true });
        const s = document.createElement('script');
        s.textContent = 'window.__inline = 1';
        document.head.append(s);
        setTimeout(() => resolve({ ran: !!window.__inline, blocked: null }), 1000);
      }),
  );
  assert.equal(result.ran, false, 'inline script must not run');
  assert.match(result.blocked ?? '', /script-src/);
});

test('no network: fetch to the web fails', async () => {
  const failed = await page.evaluate(() => fetch('https://example.org/').then(() => false, () => true));
  assert.equal(failed, true);
});

test('path traversal over app:// gets nothing', async () => {
  const status = await page.evaluate(async () => (await fetch('/%2e%2e/%2e%2e/%2e%2e/etc/passwd')).status);
  assert.equal(status, 404);
});

test('window.open is denied', async () => {
  const opened = await page.evaluate(() => window.open('file:///etc/passwd') !== null);
  assert.equal(opened, false);
  assert.equal(await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().length), 1);
});

test('navigation away is blocked', async () => {
  await page.evaluate(() => {
    location.href = 'https://example.org/';
  });
  await page.waitForTimeout(1000);
  assert.equal(await page.evaluate(() => location.origin), 'app://mvs');
});

test('permissions are refused', async () => {
  const mic = await page.evaluate(() => navigator.mediaDevices?.getUserMedia({ audio: true }).then(() => 'granted', (e) => e.name) ?? 'no-api');
  assert.notEqual(mic, 'granted');
  const notif = await page.evaluate(() => Notification.requestPermission());
  assert.notEqual(notif, 'granted');
});
