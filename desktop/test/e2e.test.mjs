// Launches the real app (Electron + ui/dist) and checks the security settings
// from the inside. Needs a display: run in Docker with
//   docker compose run --rm test
import test, { after, before } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { _electron as electron } from 'playwright-core';

const FAKE_CORE = fileURLToPath(new URL('./fake-core.js', import.meta.url));

let app;
let page;

before(async () => {
  app = await electron.launch({ args: ['.'], env: { ...process.env, MVS_DEMO: '1', MVS_CORE_PATH: FAKE_CORE } });
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

test('the page sees only copyText, invoke and onEvent', async () => {
  const keys = await page.evaluate(() => Object.keys(window.mvs).sort());
  assert.deepEqual(keys, ['copyText', 'invoke', 'onEvent']);
  assert.equal(await page.evaluate(() => typeof window.ipcRenderer), 'undefined');
});

test('copyText writes plain text only, through main', async () => {
  const link = 'mvs://join/' + 'a'.repeat(64) + '/' + 'b'.repeat(64) + '/' + 'e'.repeat(64);
  assert.equal(await page.evaluate((l) => window.mvs.copyText(l), link), true);
  assert.equal(await app.evaluate(({ clipboard }) => clipboard.readText()), link);
  // Too long or empty: refused, the clipboard is left alone.
  assert.equal(await page.evaluate(() => window.mvs.copyText('x'.repeat(5000))), false);
  assert.equal(await page.evaluate(() => window.mvs.copyText('')), false);
  assert.equal(await app.evaluate(({ clipboard }) => clipboard.readText()), link);
  // The page itself still has no clipboard permission.
  const web = await page.evaluate(() => navigator.clipboard.writeText('web').then(() => 'ok', () => 'denied'));
  assert.equal(web, 'denied');
});

test('allowed commands reach the core with named args and the data dir', async () => {
  const res = await page.evaluate(() => window.mvs.invoke('getAppInfo', []));
  assert.equal(res.ok.argv[0], '--data-dir');
  assert.match(res.ok.argv[1], /core$/);
  assert.deepEqual(res.ok.args, {});
});

test('commands outside the allow-list are refused in main', async () => {
  for (const cmd of ['grantRole', 'syncNow', 'constructor', '__proto__']) {
    assert.deepEqual(await page.evaluate((c) => window.mvs.invoke(c, []), cmd), { error: { code: 'not-allowed' } }, cmd);
  }
  assert.deepEqual(await page.evaluate(() => window.mvs.invoke('getIdentity', ['extra'])), { error: { code: 'bad-request' } });
});

test('core events reach the page', async () => {
  const got = await page.evaluate(
    () =>
      new Promise((resolve) => {
        const off = window.mvs.onEvent((name, payload) => {
          off();
          resolve([name, payload]);
        });
        window.mvs.invoke('joinServer', ['mvs://test']);
      }),
  );
  assert.deepEqual(got, ['join.status', { link: 'mvs://test', state: 'searching' }]);
});
