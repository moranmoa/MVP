// Electron main process of the desktop app (B1). It shows the UI build and
// runs the core (mvs-core) as a sidecar process; the page reaches the core
// only through preload.js -> 'core:invoke' -> the allow-list in commands.js.
//
// Security model (ui/SECURITY.md, GAP-17): the renderer is an untrusted web
// page. It runs sandboxed with context isolation and no Node, is served only
// from app://mvs with the UI's CSP as a header, cannot navigate, open
// windows, embed webviews or get permissions, and opens external links only
// through the OS after the user confirms.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { app, BrowserWindow, Menu, crashReporter, dialog, ipcMain, protocol, session, shell } = require('electron');
const sec = require('./security');
const { toRequest } = require('./commands');
const { Sidecar } = require('./sidecar');

// One instance only: a second launch (e.g. an mvs:// link, B1-3) focuses this one.
if (!app.requestSingleInstanceLock()) {
  app.exit(0);
}
// A release build never accepts debugger switches (the fuses in B1-3 also
// switch off --inspect and NODE_OPTIONS).
if (app.isPackaged && process.argv.some((a) => /^--(inspect|remote-debugging|js-flags)/.test(a))) {
  app.exit(1);
}

// Before ready: app:// behaves like a real origin (standard + secure), so the
// CSP 'self' and fetch work, and nothing else gets privileges.
protocol.registerSchemesAsPrivileged([
  { scheme: sec.SCHEME, privileges: { standard: true, secure: true, supportFetchAPI: true, stream: true } },
]);
app.enableSandbox();
// Crash dumps stay on this computer; nothing is ever uploaded.
crashReporter.start({ uploadToServer: false });

/** mvs-core: next to the app when packaged; MVS_CORE_PATH (absolute) in a checkout. */
const CORE_EXE = app.isPackaged
  ? path.join(process.resourcesPath, process.platform === 'win32' ? 'mvs-core.exe' : 'mvs-core')
  : process.env.MVS_CORE_PATH;
let core = null;

function startCore() {
  if (!CORE_EXE || !path.isAbsolute(CORE_EXE) || !fs.existsSync(CORE_EXE)) {
    console.warn('mvs-core not found; core commands answer core-unavailable');
    return;
  }
  core = new Sidecar({
    exe: CORE_EXE,
    dataDir: path.join(app.getPath('userData'), 'core'),
    onEvent: (name, payload) => {
      for (const win of BrowserWindow.getAllWindows()) {
        if (sec.isInternalUrl(win.webContents.getURL())) win.webContents.send('core:event', name, payload);
      }
    },
    // stderr only (counts and error kinds); stdout is never logged.
    log: (line) => console.log(line),
  }).start();
}

/** The renderer's only door to the core. */
function serveCoreIpc() {
  ipcMain.handle('core:invoke', async (event, cmd, args) => {
    const frame = event.senderFrame;
    // Only our own page, top frame, may ask.
    if (!frame || frame !== event.sender.mainFrame || !sec.isInternalUrl(frame.url)) return { error: { code: 'not-allowed' } };
    const req = toRequest(cmd, args);
    if (req.error) return { error: { code: req.error } };
    if (!core) return { error: { code: 'core-unavailable' } };
    return core.request(req.cmd, req.args);
  });
}

/** The built UI: next to the app when packaged, ../ui/dist (or MVS_UI_DIR) in a checkout. */
const UI_ROOT = app.isPackaged
  ? path.join(process.resourcesPath, 'ui')
  : process.env.MVS_UI_DIR || path.join(__dirname, '..', '..', 'ui', 'dist');

function serveUi() {
  const csp = sec.cspFromIndexHtml(fs.readFileSync(path.join(UI_ROOT, 'index.html'), 'utf8'));
  protocol.handle(sec.SCHEME, async (request) => {
    const file = sec.resolveAppPath(UI_ROOT, request.url);
    if (!file || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      return new Response('not found', { status: 404, headers: sec.responseHeaders(csp, 'text/plain') });
    }
    return new Response(await fs.promises.readFile(file), { status: 200, headers: sec.responseHeaders(csp, sec.mimeOf(file)) });
  });
}

/** Asks before handing a link to the OS browser. */
async function openExternal(win, url) {
  if (!sec.isExternalWebUrl(url)) return;
  const { response } = await dialog.showMessageBox(win, {
    type: 'question',
    buttons: ['Open', 'Cancel'],
    defaultId: 1,
    cancelId: 1,
    message: 'Open this link in your browser?',
    detail: url,
  });
  if (response === 0) await shell.openExternal(url);
}

/** Applied to every webContents, including any created later. */
function lockDown(contents) {
  contents.setWindowOpenHandler(({ url }) => {
    const win = BrowserWindow.fromWebContents(contents);
    if (win) openExternal(win, url).catch(() => {});
    return { action: 'deny' };
  });
  contents.on('will-navigate', (event, url) => {
    if (!sec.isInternalUrl(url)) event.preventDefault();
  });
  contents.on('will-redirect', (event, url) => {
    if (!sec.isInternalUrl(url)) event.preventDefault();
  });
  contents.on('will-attach-webview', (event) => event.preventDefault());
}

function lockDownSession(ses) {
  ses.setPermissionRequestHandler((_wc, permission, done) => done(sec.ALLOWED_PERMISSIONS.has(permission)));
  ses.setPermissionCheckHandler((_wc, permission) => sec.ALLOWED_PERMISSIONS.has(permission));
  ses.setDevicePermissionHandler(() => false);
  ses.setDisplayMediaRequestHandler((_req, done) => done({}));
  // Nothing the UI needs lives on the network; any other request is a bug or an attack.
  ses.webRequest.onBeforeRequest((details, done) => {
    const ok = details.url.startsWith(`${sec.ORIGIN}/`) || details.url.startsWith('devtools://') || details.url.startsWith('data:') || details.url.startsWith('blob:');
    done({ cancel: !ok });
  });
}

function createWindow() {
  const win = new BrowserWindow({
    width: 1280,
    height: 800,
    minWidth: 360,
    minHeight: 480,
    show: false,
    backgroundColor: '#1e1f22',
    webPreferences: { ...sec.WEB_PREFERENCES, preload: path.join(__dirname, 'preload.js'), devTools: !app.isPackaged },
  });
  win.once('ready-to-show', () => win.show());
  // The mock core stays reachable with ?demo while the real core is wired up (B1-2).
  const demo = process.env.MVS_DEMO ? `?demo=${encodeURIComponent(process.env.MVS_DEMO)}` : '';
  win.loadURL(`${sec.ORIGIN}/index.html${demo}`);
  return win;
}

app.on('web-contents-created', (_e, contents) => lockDown(contents));

app.whenReady().then(() => {
  // No default menu (and with it no DevTools) in a release build.
  if (app.isPackaged) Menu.setApplicationMenu(null);
  lockDownSession(session.defaultSession);
  serveUi();
  serveCoreIpc();
  startCore();
  createWindow();
  app.on('activate', () => BrowserWindow.getAllWindows().length === 0 && createWindow());
});

app.on('second-instance', () => {
  const win = BrowserWindow.getAllWindows()[0];
  if (win) {
    if (win.isMinimized()) win.restore();
    win.focus();
  }
});

app.on('will-quit', () => core?.stop());

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
