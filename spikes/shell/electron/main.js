// Electron shell for the footprint comparison (experiment 6): the unchanged
// ui/ build with the mock core, nothing else.
//
// SHELL_BENCH_OUT=<file>  inject ../bench.js and append its marks to <file>.

const { app, BrowserWindow } = require('electron');
const fs = require('node:fs');
const path = require('node:path');

const benchOut = process.env.SHELL_BENCH_OUT;
const mark = (label) => fs.appendFileSync(benchOut, `${label} ${Date.now()}\n`);

app.whenReady().then(() => {
  const win = new BrowserWindow({
    width: 1280,
    height: 800,
    webPreferences: { contextIsolation: true, sandbox: true },
  });
  const res = app.isPackaged ? process.resourcesPath : path.join(__dirname, '..');
  if (benchOut) {
    win.webContents.on('console-message', (e) => {
      const m = /^BENCH_MARK (\S+)/.exec(e.message);
      if (m) mark(m[1]);
    });
    win.webContents.on('did-finish-load', () => {
      win.webContents.executeJavaScript(fs.readFileSync(path.join(res, 'bench.js'), 'utf8'));
    });
  }
  const ui = app.isPackaged ? path.join(res, 'ui') : path.join(__dirname, '..', '..', '..', 'ui', 'dist');
  win.loadFile(path.join(ui, 'index.html'), { query: { demo: '1', still: '1' } });
});

app.on('window-all-closed', () => app.quit());
