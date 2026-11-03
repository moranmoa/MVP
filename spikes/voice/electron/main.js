// Electron variant of the voice spike: the same web client (../web), run on
// Electron's Chromium WebRTC stack.
//
//   npm start -- --signal=ws://host:8080 --room=test --name=alice
//
// Any --key=value argument becomes a URL parameter of the client (see
// ../web/voice.js for the list).

const { app, BrowserWindow, session } = require('electron');
const path = require('node:path');

const query = {};
for (const arg of process.argv.slice(1)) {
  const m = /^--(\w+)=(.*)$/.exec(arg);
  if (m) query[m[1]] = m[2];
}

// Bench runs (synthetic tone, no microphone permission) would otherwise hide
// host candidates behind mDNS names, which the str0m peer cannot resolve.
// With a real microphone Chromium exposes them anyway.
if (query.src === 'pulses') app.commandLine.appendSwitch('disable-features', 'WebRtcHideLocalIpsWithMdns');

app.whenReady().then(() => {
  // Microphone and speaker choice; nothing else is granted.
  const allowed = new Set(['media', 'speaker-selection']);
  session.defaultSession.setPermissionRequestHandler((_wc, permission, done) => done(allowed.has(permission)));
  session.defaultSession.setPermissionCheckHandler((_wc, permission) => allowed.has(permission));

  const win = new BrowserWindow({
    width: 820,
    height: 900,
    webPreferences: { contextIsolation: true, sandbox: true },
  });
  // In the packaged app the web client sits in resources/web, next to app.asar.
  const web = app.isPackaged ? path.join(process.resourcesPath, 'web') : path.join(__dirname, '..', 'web');
  win.loadFile(path.join(web, 'index.html'), { query });
});

app.on('window-all-closed', () => app.quit());
