// Electron main process of the desktop app (B1). It shows the UI build and
// runs the core (mvs-core) as a sidecar process; the page reaches the core
// only through preload.js -> 'core:invoke' -> the allow-list in commands.js.
//
// Security model (ui/SECURITY.md, GAP-17): the renderer is an untrusted web
// page. It runs sandboxed with context isolation and no Node, is served only
// from app://mvs with the UI's CSP as a header, cannot navigate, open
// windows, embed webviews or get permissions (except the microphone, for its
// own top frame), and opens external links only through the OS after the user
// confirms.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { app, BrowserWindow, Menu, Notification, clipboard, crashReporter, dialog, ipcMain, protocol, session, shell } = require('electron');
const sec = require('./security');
const { toRequest } = require('./commands');
const signout = require('./signout');
const { Sidecar } = require('./sidecar');
const deeplink = require('./deeplink');
const { relayFor, bootstrapFor, logHost, stunFor, stunHost } = require('./config');
const update = require('./update');
const { createLog } = require('./logfile');
const { createNotifier } = require('./notify');

// Tests only, and only in a checkout (never a release build): a throwaway
// profile, so a test run never shares the installed app's profile
// (%APPDATA%\MVS, ~/.config/MVS) or its single-instance lock. Set before the
// lock below, which lives in the profile.
if (!app.isPackaged && process.env.MVS_TEST_USER_DATA && path.isAbsolute(process.env.MVS_TEST_USER_DATA)) {
  app.setPath('userData', process.env.MVS_TEST_USER_DATA);
}

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
/** Main's log: the console and a capped file in the OS logs folder (T8-01). */
const fileLog = createLog(app.getPath('logs'));
const log = (line) => {
  console.log(line);
  fileLog(line);
};
/** Where the core stands, for the "core unavailable" screen (T8-01). */
let coreStatus = { state: 'starting' };
/**
 * Our STUN server for the page's WebRTC (#222), or null. It rides on every
 * core.status (read-only; only our own pages get core.status), so the page
 * never asks for it. Its host never reaches the log.
 */
const STUN_URL = stunFor({ packaged: app.isPackaged, resourcesPath: process.resourcesPath });
if (stunHost(STUN_URL)) fileLog.hide(stunHost(STUN_URL));
const statusForPage = (status) => ({ ...status, stunUrl: STUN_URL });
/** V-C2: the microphone only while we are in a call (security.js). */
const callGate = sec.createCallGate();

/** Sends a shell event (not from the core) to our own windows. */
function toPages(name, payload) {
  for (const win of BrowserWindow.getAllWindows()) {
    if (sec.isInternalUrl(win.webContents.getURL())) win.webContents.send('core:event', name, payload);
  }
}

function setCoreStatus(status) {
  callGate.onCoreStatus(status);
  coreStatus = status;
  toPages('core.status', statusForPage(status));
}

// In-app update (B8): on only in an installed copy that has mvs-launcher; the
// root is derived from this exe's location, never trusted from argv (update.js).
const UPDATE = update.resolveUpdate({ packaged: app.isPackaged, execPath: process.execPath, argv: process.argv });

function startCore() {
  if (!CORE_EXE || !path.isAbsolute(CORE_EXE) || !fs.existsSync(CORE_EXE)) {
    log('mvs-core not found; core commands answer core-unavailable');
    setCoreStatus({ state: 'down', detail: 'missing' });
    return;
  }
  // The core's folder is the user's only (0700); on Windows the per-user
  // profile ACL already does this.
  const dataDir = path.join(app.getPath('userData'), 'core');
  fs.mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  if (process.platform !== 'win32') fs.chmodSync(dataDir, 0o700);
  const relay = relayFor({ packaged: app.isPackaged, resourcesPath: process.resourcesPath });
  // The relay's host (DNS name or IP) must never reach the log, not even
  // bare; nor its direct addresses (#219).
  const bootstrap = bootstrapFor({ packaged: app.isPackaged, resourcesPath: process.resourcesPath });
  for (const addr of [relay, ...bootstrap]) {
    const host = logHost(addr);
    if (host) fileLog.hide(host);
  }
  // Checkouts only (tests): Windows cannot execute a .js stand-in for the core
  // (EFTYPE), so it runs under this Electron in Node mode. Never in a packaged
  // app, where the RunAsNode fuse is off anyway.
  const jsCore = !app.isPackaged && process.platform === 'win32' && CORE_EXE.endsWith('.js');
  core = new Sidecar({
    exe: jsCore ? process.execPath : CORE_EXE,
    script: jsCore ? CORE_EXE : null,
    dataDir,
    // Without a relay the core runs offline (no invites, no chat with others).
    relay,
    bootstrap,
    extraArgs: update.coreArgs(UPDATE),
    onEvent: (name, payload) => {
      callGate.onEvent(name, payload);
      toPages(name, payload);
    },
    onStatus: setCoreStatus,
    // stderr only (counts and error kinds); stdout is never logged.
    log,
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
    // "Restart to update": the core's applyUpdate (refused during a call),
    // then a relaunch through the launcher, which starts the staged version.
    if (req.cmd === 'restartToUpdate' && UPDATE) {
      return update.restartToUpdate(UPDATE, (c, a) => core.request(c, a), () => {
        app.relaunch({ execPath: UPDATE.launcher, args: [] });
        app.quit();
      });
    }
    // #304: without the words, a native confirm first; after ok, the page's
    // storage and the log go too (see signout.js).
    if (req.cmd === 'signOut') {
      const win = BrowserWindow.fromWebContents(event.sender);
      return signout.signOut({
        args: req.args,
        confirmWithoutWords: async () => (await dialog.showMessageBox(win, signout.WITHOUT_WORDS_DIALOG)).response === 1,
        request: () => core.request(req.cmd, req.args),
        clearStorage: async () => {
          await event.sender.session.clearStorageData({
            origin: sec.ORIGIN,
            storages: ['localstorage', 'indexdb', 'cachestorage', 'serviceworkers', 'cookies', 'websql', 'shadercache'],
          });
          await event.sender.session.clearCache();
        },
        clearLogs: () => signout.clearLogFiles(app.getPath('logs')),
      });
    }
    // The call gate sees joinCall's answer before the page does, so the
    // page's getUserMedia after a join finds the microphone open.
    const token = callGate.beforeCommand(req.cmd);
    const res = await core.request(req.cmd, req.args);
    callGate.afterCommand(req.cmd, token, res);
    return res;
  });
  // The "core unavailable" screen (T8-01): try again now, and open the logs.
  const fromUs = (event) => {
    const frame = event.senderFrame;
    return !!frame && frame === event.sender.mainFrame && sec.isInternalUrl(frame.url);
  };
  let lastRetry = 0;
  ipcMain.handle('core:retry', (event) => {
    if (!fromUs(event)) return false;
    // At most once every 5 s, whatever the page does.
    if (Date.now() - lastRetry < 5000) return false;
    lastRetry = Date.now();
    if (core) return core.retryNow();
    startCore(); // the binary may have been put back since
    return !!core;
  });
  // G-61: a desktop notification, from our own top frame only. Main shows
  // it (the page's notifications permission stays denied); a click brings
  // the window up and the page opens the channel.
  const notifier = createNotifier({
    Notification,
    onClick: (target) => {
      const win = BrowserWindow.getAllWindows()[0];
      if (win) {
        if (win.isMinimized()) win.restore();
        win.show();
        win.focus();
      }
      toPages('notify.click', target);
    },
  });
  // Our own title bar (docs/ux/specs/desktop-chrome.md): one command for
  // the window, from our top frame, on the sender's own window only.
  // minimize (to the taskbar; there is no tray), toggle-maximize, close (the
  // page asks first during a call), and on Windows the overlay buttons'
  // colours for the app's theme (fixed pairs, never colours from the page).
  ipcMain.handle('window:control', (event, action) => {
    if (!fromUs(event) || FRAME === 'native') return false;
    const win = BrowserWindow.fromWebContents(event.sender);
    if (!win) return false;
    if (action === 'minimize') win.minimize();
    else if (action === 'toggle-maximize') win.isMaximized() ? win.unmaximize() : win.maximize();
    else if (action === 'close') {
      closeConfirmed.add(win);
      win.close();
    } else if ((action === 'theme:light' || action === 'theme:dark') && FRAME === 'overlay') win.setTitleBarOverlay(OVERLAY[action.slice(6)]);
    else return false;
    return true;
  });
  ipcMain.handle('notify:show', (event, notice) => {
    if (!fromUs(event)) return false;
    return notifier.show(notice);
  });
  ipcMain.handle('logs:open', async (event) => {
    if (!fromUs(event)) return false;
    return (await shell.openPath(app.getPath('logs'))) === '';
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
  // Voice: WebRTC gathers candidates only on the default route's interface
  // (public and private address), never every local adapter (VPNs, VMs).
  contents.setWebRTCIPHandlingPolicy(sec.WEBRTC_IP_POLICY);
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
  // Voice v1: the microphone (audio only) for our own top frame, and only
  // during a call (V-C2); nothing else. Camera and screen capture stay refused (rules in security.js).
  // A destroyed or unreadable webContents gives '' (never internal).
  const urlOf = (wc) => {
    try {
      return wc.isDestroyed() ? '' : wc.getURL();
    } catch {
      return '';
    }
  };
  ses.setPermissionRequestHandler((wc, permission, done, details) => {
    let ok = false;
    try {
      ok = sec.allowPermissionRequest(wc ? urlOf(wc) : '', permission, details, callGate.inCall) === true;
    } catch {
      ok = false;
    }
    done(ok);
  });
  ses.setPermissionCheckHandler((wc, permission, requestingOrigin, details) => {
    try {
      return sec.allowPermissionCheck(wc ? urlOf(wc) : undefined, permission, requestingOrigin, details, callGate.inCall) === true;
    } catch {
      return false;
    }
  });
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
let healthReported = false;
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

/**
 * Our own title bar (docs/ux/specs/desktop-chrome.md §2), 32px, drawn by the
 * page. Windows: hidden title bar with the native buttons on an overlay
 * (Snap Layouts keep working). Linux: frameless, the page draws the buttons.
 * macOS keeps its native frame for now.
 */
const FRAME = process.platform === 'win32' ? 'overlay' : process.platform === 'linux' ? 'custom' : 'native';
/** The overlay buttons on --bg-rail, per theme (ui/src/styles/theme.css). */
const OVERLAY = {
  light: { color: '#dde0e6', symbolColor: '#1d2129', height: 32 },
  dark: { color: '#121418', symbolColor: '#e6e8ec', height: 32 },
};
const FRAME_OPTIONS = {
  overlay: { titleBarStyle: 'hidden', titleBarOverlay: OVERLAY.light },
  custom: { frame: false },
  native: {},
}[FRAME];
/** The window icon (Linux; Windows takes the exe's). */
const ICON = app.isPackaged ? path.join(process.resourcesPath, 'icon.png') : path.join(__dirname, '..', 'build', 'icons', '512x512.png');
/** Windows whose close the page confirmed (during a call, it asks first). */
const closeConfirmed = new WeakSet();
/**
 * When a close was last held for the page to ask. A second close within
 * this long always goes through, so a hung or hostile page can never keep
 * the app (and the call's microphone) open (Security Lead, #358).
 */
const closeHeldAt = new WeakMap();
const CLOSE_AGAIN_MS = 10_000;

function createWindow() {
  const win = new BrowserWindow({
    ...FRAME_OPTIONS,
    ...(fs.existsSync(ICON) ? { icon: ICON } : {}),
    width: 1280,
    height: 800,
    minWidth: 360,
    minHeight: 480,
    show: false,
    backgroundColor: '#1e1f22',
    webPreferences: { ...sec.WEB_PREFERENCES, preload: path.join(__dirname, 'preload.js'), devTools: !app.isPackaged },
  });
  win.once('ready-to-show', () => win.show());
  // The title bar shows maximize or restore, and hides in fullscreen.
  const sendState = () => {
    if (sec.isInternalUrl(win.webContents.getURL())) {
      win.webContents.send('core:event', 'window.state', { maximized: win.isMaximized(), fullScreen: win.isFullScreen() });
    }
  };
  for (const e of ['maximize', 'unmaximize', 'enter-full-screen', 'leave-full-screen']) win.on(e, sendState);
  // Close quits; during a call the page asks first ("This will end your
  // call. Close MVS?") and confirms with window:control close.
  // A second close within 10 s closes whatever the page does.
  win.on('close', (e) => {
    if (!callGate.inCall) return;
    const held = closeHeldAt.get(win);
    const askPage = FRAME !== 'native' && !closeConfirmed.has(win) && sec.isInternalUrl(win.webContents.getURL());
    if (askPage && (held === undefined || Date.now() - held > CLOSE_AGAIN_MS)) {
      closeHeldAt.set(win, Date.now());
      e.preventDefault();
      win.webContents.send('core:event', 'window.closeRequest', {});
      return;
    }
    // Closing during a call ends it here, not by trusting the page: the
    // microphone gate shuts now and the core leaves the call.
    callGate.end();
    core?.request('leaveCall', {}).catch(() => {});
  });
  // Every load (a reload too) gets the current core status.
  win.webContents.on('did-finish-load', () => {
    flushInvite();
    // The page may have missed earlier changes; the preload keeps this for the UI.
    if (sec.isInternalUrl(win.webContents.getURL())) win.webContents.send('core:event', 'core.status', statusForPage(coreStatus));
    // Commit this version once the UI is up and the core answers (main only, once).
    if (!healthReported) {
      healthReported = true;
      update.reportHealthy(UPDATE, (cmd, args) => (core ? core.request(cmd, args) : Promise.resolve({ error: { code: 'core-unavailable' } })));
    }
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
  // With updates on, links go to the launcher, which starts the healthy version.
  if (app.isPackaged) app.setAsDefaultProtocolClient(deeplink.SCHEME, UPDATE ? UPDATE.launcher : process.execPath, ['--']);
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

app.on('will-quit', () => {
  callGate.end();
  core?.stop();
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
