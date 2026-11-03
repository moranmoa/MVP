// The only thing the page gets from the shell: `window.mvs.invoke` and
// `window.mvs.onEvent`. ipcRenderer itself is never exposed. The main
// process checks every call (sender, allow-list, size); see main.js.
'use strict';

const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('mvs', {
  /** Resolves with { ok } or { error: { code, positions? } }. */
  invoke: (cmd, args) => ipcRenderer.invoke('core:invoke', String(cmd), Array.isArray(args) ? args : []),
  /** Calls back with (eventName, payload) for every core event; returns an unsubscribe. */
  onEvent: (callback) => {
    if (typeof callback !== 'function') return () => {};
    const handler = (_e, name, payload) => callback(name, payload);
    ipcRenderer.on('core:event', handler);
    return () => ipcRenderer.removeListener('core:event', handler);
  },
});
