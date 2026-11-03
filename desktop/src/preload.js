// The only thing the page gets from the shell: `window.mvs.invoke` and
// `window.mvs.onEvent`. ipcRenderer itself is never exposed. The main
// process checks every call (sender, allow-list, size); see main.js.
'use strict';

const { contextBridge, ipcRenderer } = require('electron');

const listeners = new Set();
// A `link.open` (an mvs:// invite) can arrive before the UI has subscribed,
// e.g. on the first launch; the latest one is kept for the first subscriber.
let pendingLink = null;

ipcRenderer.on('core:event', (_e, name, payload) => {
  if (name === 'link.open' && listeners.size === 0) {
    pendingLink = payload;
    return;
  }
  for (const cb of listeners) cb(name, payload);
});

contextBridge.exposeInMainWorld('mvs', {
  /** Resolves with { ok } or { error: { code, positions? } }. */
  invoke: (cmd, args) => ipcRenderer.invoke('core:invoke', String(cmd), Array.isArray(args) ? args : []),
  /**
   * Calls back with (eventName, payload) for every core event, plus
   * `link.open` { link } when the OS opens an mvs:// invite. Returns an
   * unsubscribe.
   */
  onEvent: (callback) => {
    if (typeof callback !== 'function') return () => {};
    listeners.add(callback);
    if (pendingLink) {
      const p = pendingLink;
      pendingLink = null;
      setTimeout(() => callback('link.open', p), 0);
    }
    return () => listeners.delete(callback);
  },
});
