// The only thing the page gets from the shell: `window.mvs.invoke` and
// `window.mvs.onEvent`. ipcRenderer itself is never exposed. The main
// process checks every call (sender, allow-list, size); see main.js.
'use strict';

const { contextBridge, ipcRenderer } = require('electron');

const listeners = new Set();
// Shell events can arrive before the UI has subscribed: `link.open` (an
// mvs:// invite, e.g. on the first launch) and `core.status` (sent at load).
// The latest of each is kept for the first subscriber.
const KEPT = new Set(['link.open', 'core.status']);
const pending = new Map();

ipcRenderer.on('core:event', (_e, name, payload) => {
  if (KEPT.has(name) && listeners.size === 0) {
    pending.set(name, payload);
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
  /** Puts plain text on the clipboard (e.g. an invite link). Resolves true if it did. */
  copyText: (text) => ipcRenderer.invoke('clipboard:write', String(text)),
  /** "Core unavailable" screen: start the core again now. Status follows as core.status. */
  retryCore: () => ipcRenderer.invoke('core:retry'),
  /** Opens the shell's logs folder (no words, no message content). Resolves true if it did. */
  openLogsFolder: () => ipcRenderer.invoke('logs:open'),
  onEvent: (callback) => {
    if (typeof callback !== 'function') return () => {};
    listeners.add(callback);
    for (const [name, payload] of pending) setTimeout(() => callback(name, payload), 0);
    pending.clear();
    return () => listeners.delete(callback);
  },
});
