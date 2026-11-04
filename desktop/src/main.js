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
const { app, BrowserWindow, Menu, clipboard, crashReporter, dialog, ipcMain, protocol, session, shell } = require('electron');
const sec = require('./security');
const { toRequest } = require('./commands');
const { Sidecar } = require('./sidecar');
const deeplink = require('./deeplink');
const { relayFor } = require('./config');

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
/** Where the core stands, for the "core unavailable" screen (T8-01). */
let coreStatus = { state: 'starting' };

/** Sends a shell event (not from the core) to our own windows. */
function toPages(name, payload) {
  for (const win of BrowserWindow.getAllWindows()) {
    if (sec.isInternalUrl(win.webContents.getURL())) win.webContents.send('core:event', name, payload);
  }
}

function setCoreStatus(status) {
  coreStatus = status;
  toPages('core.status', status);
}

function startCore() {
  if (!CORE_EXE || !path.isAbsolute(CORE_EXE) || !fs.existsSync(CORE_EXE)) {
    console.warn('mvs-core not found; core commands answer core-unavailable');
    setCoreStatus({ state: 'down', detail: 'missing' });
    return;
  }
  // The core's folder is the user's only (0700); on Windows the per-user
  // profile ACL already does this.
  const dataDir = path.join(app.getPath('userData'), 'core');
  fs.mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  if (process.platform !== 'win32') fs.chmodSync(dataDir, 0o700);
  core = new Sidecar({
    exe: CORE_EXE,
    dataDir,
    // Without a relay the core runs offline (no invites, no chat with others).
    relay: relayFor({ packaged: app.isPackaged, resourcesPath: process.resourcesPath }),
    onEvent: toPages,
    onStatus: setCoreStatus,
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
    if (!core) return { error: { code: 'core-unavailable', detail: 'missing' } };
    return core.request(req.cmd, req.args);
  });
  // Copy text for the user (an invite link). Web clipboard permissions stay
  // denied; this writes plain text only, from our own top frame, up to 4 KiB.
  ipcMain.handle('clipboard:write', (event, text) => {
    const frame = event.senderFrame;
    if (!frame || frame !== event.sender.mainFrame || !sec.isInternalUrl(frame.url)) return false;
    if (typeof text !== 'string' || text.length === 0 || text.length > 4096) return false;
    clipboard.writeText(text);
    return true;
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
    // The parsed form shows punycode, so look-alike hosts are visible.
    detail: new URL(url).href,
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
    const ok = details.url.startsWith(`${sec.ORIGIN}/`) || (!app.isPackaged && details.url.startsWith('devtools://')) || details.url.startsWith('data:') || details.url.startsWith('blob:');
    done({ cancel: !ok });
  });
}

/**
 * An mvs:// invite from the OS. Only a link of the exact invite format gets
 * through (deeplink.js). Main never joins by itself: a link from outside
 * must not reveal who we are to someone's server without a click. It sends
 * the UI a `link.open` event; the UI shows the join screen, pre-filled, and
 * calls joinServer only when the user presses Join.
 */
let pendingInvite = null;
let uiReady = false;

function openInvite(raw) {
  const link = deeplink.parseInviteLink(raw);
  if (!link) return;
  const win = BrowserWindow.getAllWindows()[0];
  if (win) {
    if (win.isMinimized()) win.restore();
    win.focus();
  }
  if (!uiReady || !win || !sec.isInternalUrl(win.webContents.getURL())) {
    pendingInvite = link;
    return;
  }
  win.webContents.send('core:event', 'link.open', { link });
}

function flushInvite() {
  uiReady = true;
  if (pendingInvite) {
    const link = pendingInvite;
    pendingInvite = null;
    openInvite(link);
  }
}

// macOS delivers links as an event (also before ready).
app.on('open-url', (event, url) => {
  event.preventDefault();
  openInvite(url);
});

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
  // Every load (a reload too) gets the current core status.
  win.webContents.on('did-finish-load', () => {
    flushInvite();
    // The page may have missed earlier changes; the preload keeps this for the UI.
    if (sec.isInternalUrl(win.webContents.getURL())) win.webContents.send('core:event', 'core.status', coreStatus);
  });
  // The mock demo (?demo) is for checkouts only, never a release build.
  const demo = !app.isPackaged && process.env.MVS_DEMO ? `?demo=${encodeURIComponent(process.env.MVS_DEMO)}` : '';
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
  // The OS sends mvs:// links to the installed app only; a checkout never registers.
  // `--` before the link: whatever the link holds can never become a switch.
  if (app.isPackaged) app.setAsDefaultProtocolClient(deeplink.SCHEME, process.execPath, ['--']);
  startCore();
  createWindow();
  const first = deeplink.inviteFromArgv(process.argv);
  if (first) openInvite(first);
  app.on('activate', () => BrowserWindow.getAllWindows().length === 0 && createWindow());
});

app.on('second-instance', (_event, argv) => {
  const win = BrowserWindow.getAllWindows()[0];
  if (win) {
    if (win.isMinimized()) win.restore();
    win.focus();
  }
  const link = deeplink.inviteFromArgv(argv);
  if (link) openInvite(link);
});

app.on('will-quit', () => core?.stop());

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
