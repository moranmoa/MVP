// Runs the core (`mvs-core`, core/api) as a child process and talks to it in
// JSON lines over stdin/stdout. No Electron imports: test/sidecar.test.js
// drives it with a fake core.
//
// Rules (security lead, B1):
//   - spawned by an absolute path, no shell, a minimal environment, stdio only;
//   - stdout is never logged (createIdentity / getRecoveryWords carry words);
//   - lines are capped: 1 MiB to the core, 16 MiB from it; an oversized line
//     from the core kills it and it is restarted;
//   - every request has a timeout; a crash fails what is pending and restarts.

'use strict';

const { spawn } = require('node:child_process');
const path = require('node:path');

const MAX_OUT_LINE = 1024 * 1024;
const MAX_IN_LINE = 16 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS = 30_000;
const CRASH_LOOP = 5;
/** Commands that legitimately take long (key generation, OS auth prompts). */
const SLOW = { createIdentity: 120_000, restoreIdentity: 120_000, getRecoveryWords: 120_000 };

/** Only what the core needs to find the user's folders and the OS keychain. */
const ENV_KEEP = [
  // Windows
  'SystemRoot', 'SYSTEMROOT', 'windir', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'TEMP', 'TMP', 'USERNAME',
  // macOS / Linux
  'HOME', 'USER', 'LOGNAME', 'TMPDIR', 'LANG', 'LC_ALL',
  // Linux Secret Service (keychain) over the session bus
  'DBUS_SESSION_BUS_ADDRESS', 'XDG_RUNTIME_DIR', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME',
];

function minimalEnv(env = process.env) {
  const out = {};
  for (const k of ENV_KEEP) if (env[k] !== undefined) out[k] = env[k];
  return out;
}

class Sidecar {
  /**
   * @param {object} o
   * @param {string} o.exe absolute path of mvs-core
   * @param {string} o.dataDir absolute path of its data folder
   * @param {string|null} [o.relay] relay multiaddr, already checked (config.js)
   * @param {string[]} [o.bootstrap] the relay's direct multiaddrs, already checked (config.js)
   * @param {string[]} [o.extraArgs] further switches for mvs-core
   * @param {(name: string, payload: unknown) => void} o.onEvent
   * @param {(line: string) => void} [o.log] for stderr lines and lifecycle notes
   */
  constructor({ exe, dataDir, relay = null, bootstrap = [], extraArgs = [], onEvent, onStatus = () => {}, log = () => {}, timeouts = {}, backoffBase = 500, spawnImpl = spawn }) {
    if (!path.isAbsolute(exe)) throw new Error('mvs-core must be given by an absolute path');
    if (!path.isAbsolute(dataDir)) throw new Error('the data folder must be an absolute path');
    this.exe = exe;
    this.dataDir = dataDir;
    // Optional: the relay multiaddr (config.js checks it) and extra switches
    // from other parts of the shell (e.g. the updater); strings only.
    this.relay = relay;
    // The relay's direct addresses (config.js checks them); only with a relay.
    if (!Array.isArray(bootstrap) || bootstrap.some((a) => typeof a !== 'string')) throw new Error('bootstrap must be strings');
    this.bootstrap = bootstrap;
    if (!Array.isArray(extraArgs) || extraArgs.some((a) => typeof a !== 'string')) throw new Error('extraArgs must be strings');
    this.extraArgs = extraArgs;
    this.onEvent = onEvent;
    this.log = log;
    this.timeouts = { default: DEFAULT_TIMEOUT_MS, ...SLOW, ...timeouts };
    this.spawnImpl = spawnImpl;
    this.pending = new Map(); // id -> { resolve, timer }
    this.nextId = 1;
    this.child = null;
    this.stopped = false;
    this.restarts = 0;
    // For the "core unavailable" screen (T8-01): starting, up, or down with a reason.
    this.onStatus = onStatus;
    this.backoffBase = backoffBase;
    this.status = { state: 'starting' };
  }

  setStatus(state, detail) {
    if (this.status.state === state && this.status.detail === detail) return;
    this.status = detail ? { state, detail } : { state };
    this.onStatus(this.status);
  }

  /** The child is gone (exit, or it never started): fail what waits, retry later. */
  lost(child, detail) {
    if (this.child !== child) return;
    this.child = null;
    this.failAll('core-restarted');
    if (this.stopped) return;
    this.restarts += 1;
    // Five failures in a row without ever answering: it will not come up by itself.
    this.setStatus('down', this.restarts >= CRASH_LOOP ? 'crash-loop' : detail);
    const delay = Math.min(30_000, this.backoffBase * 2 ** Math.min(this.restarts - 1, 6));
    this.restartTimer = setTimeout(() => this.start(), delay);
  }

  start() {
    this.stopped = false;
    const args = [
      '--data-dir',
      this.dataDir,
      ...(this.relay ? ['--relay', this.relay, ...this.bootstrap.flatMap((b) => ['--bootstrap', b])] : []),
      ...this.extraArgs,
    ];
    let child;
    try {
      child = this.spawnImpl(this.exe, args, {
        shell: false,
        windowsHide: true,
        stdio: ['pipe', 'pipe', 'pipe'],
        env: minimalEnv(),
      });
    } catch (e) {
      // Some failures throw instead of emitting "error" (on Windows, EFTYPE for
      // a file it cannot execute). Same path as a failed start: down, retry
      // with backoff, and never take the caller (app start-up) down with it.
      this.log(`core failed to start: ${e.code || e.message}`);
      const none = {};
      this.child = none;
      this.lost(none, 'start-failed');
      return this;
    }
    this.child = child;
    let buf = Buffer.alloc(0);
    child.stdout.on('data', (chunk) => {
      buf = Buffer.concat([buf, chunk]);
      let nl;
      while ((nl = buf.indexOf(0x0a)) !== -1) {
        const line = buf.subarray(0, nl);
        buf = buf.subarray(nl + 1);
        if (line.length > MAX_IN_LINE) return this.kill('line from the core over 16 MiB');
        this.onLine(line.toString('utf8'));
      }
      if (buf.length > MAX_IN_LINE) this.kill('line from the core over 16 MiB');
    });
    let errBuf = '';
    child.stderr.on('data', (chunk) => {
      errBuf += chunk.toString('utf8');
      const lines = errBuf.split('\n');
      errBuf = lines.pop().slice(-4096);
      for (const l of lines) {
        if (!l) continue;
        if (l.trim() === 'mvs-core: ready') this.up();
        this.log(`core: ${l.slice(0, 1000)}`);
      }
    });
    // A spawn failure (not found, not executable) may come without an exit.
    child.on('error', (e) => {
      this.log(`core failed to start: ${e.code || e.message}`);
      this.lost(child, 'start-failed');
    });
    child.on('exit', (code, signal) => {
      if (this.child === child && !this.stopped) this.log(`core exited (${code ?? signal}); restarting`);
      this.lost(child, 'crashed');
    });
    return this;
  }

  onLine(text) {
    let msg;
    try {
      msg = JSON.parse(text);
    } catch {
      return this.log('core wrote a line that is not JSON');
    }
    if (msg && msg.type === 'event' && typeof msg.event === 'string') {
      this.up();
      this.onEvent(msg.event, msg.payload);
    } else if (msg && msg.type === 'response') {
      this.up();
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      clearTimeout(p.timer);
      p.resolve(msg.error ? { error: sanitizeError(msg.error) } : { ok: msg.ok ?? null });
    }
  }

  /** Resolves with { ok } or { error: { code, positions? } }; never rejects. */
  request(cmd, args) {
    return new Promise((resolve) => {
      if (!this.child) return resolve({ error: this.unavailable() });
      const id = this.nextId++;
      const line = `${JSON.stringify({ id, cmd, args })}\n`;
      if (Buffer.byteLength(line) > MAX_OUT_LINE) return resolve({ error: { code: 'too-large' } });
      const ms = this.timeouts[cmd] ?? this.timeouts.default;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        resolve({ error: { code: 'timeout' } });
      }, ms);
      this.pending.set(id, { resolve, timer });
      this.child.stdin.write(line);
    });
  }

  /**
   * The user pressed "try again": start now, with a fresh backoff. A running
   * core is replaced too (it may be up but failing, e.g. no keyring, QA-20).
   */
  retryNow() {
    if (this.stopped) return false;
    clearTimeout(this.restartTimer);
    if (this.child) {
      const old = this.child;
      this.child = null; // its exit is then ignored by lost()
      this.failAll('core-restarted');
      old.kill();
    }
    this.restarts = 0;
    this.setStatus('starting');
    this.start();
    return true;
  }

  up() {
    this.restarts = 0;
    this.setStatus('up');
  }

  /** The error every request gets while the core is not running. */
  unavailable() {
    return this.status.detail ? { code: 'core-unavailable', detail: this.status.detail } : { code: 'core-unavailable' };
  }

  failAll(code) {
    for (const [id, p] of this.pending) {
      clearTimeout(p.timer);
      p.resolve({ error: { code } });
      this.pending.delete(id);
    }
  }

  kill(reason) {
    this.log(`killing the core: ${reason}`);
    this.child?.kill();
  }

  stop() {
    this.stopped = true;
    clearTimeout(this.restartTimer);
    this.failAll('core-stopped');
    if (this.child) {
      this.child.stdin.end();
      const c = this.child;
      setTimeout(() => c.exitCode === null && c.kill(), 3000).unref();
    }
  }
}

/** Only the documented error fields reach the renderer. */
function sanitizeError(e) {
  const out = { code: typeof e?.code === 'string' ? e.code.slice(0, 64) : 'error' };
  if (Array.isArray(e?.positions)) out.positions = e.positions.filter((n) => Number.isInteger(n)).slice(0, 24);
  return out;
}

module.exports = { Sidecar, minimalEnv, sanitizeError, MAX_OUT_LINE, MAX_IN_LINE };
