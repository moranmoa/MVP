// Security policy of the desktop shell, kept free of Electron imports so it
// can be unit-tested with plain Node (test/security.test.js). main.js applies
// it; ui/SECURITY.md and docs/security/threat-model.md (GAP-17) explain why.

'use strict';

const path = require('node:path');

/** The UI is served from this origin only. */
const SCHEME = 'app';
const HOST = 'mvs';
const ORIGIN = `${SCHEME}://${HOST}`;

/** Every renderer gets exactly these; nothing may loosen them. */
const WEB_PREFERENCES = Object.freeze({
  sandbox: true,
  contextIsolation: true,
  nodeIntegration: false,
  nodeIntegrationInWorker: false,
  nodeIntegrationInSubFrames: false,
  webSecurity: true,
  allowRunningInsecureContent: false,
  webviewTag: false,
  experimentalFeatures: false,
  enableBlinkFeatures: '',
  navigateOnDragDrop: false,
  spellcheck: false,
  safeDialogs: true,
});

/**
 * The CSP is defined once, in ui/csp.ts, and Vite writes it into the built
 * index.html as a <meta> tag. The shell sends the same policy as a header and
 * adds what only a header can carry. No policy in the page = refuse to start.
 */
function cspFromIndexHtml(html) {
  const tag = [...html.matchAll(/<meta\b[^>]*>/gi)]
    .map((m) => m[0])
    .find((t) => /http-equiv\s*=\s*(["'])Content-Security-Policy\1/i.test(t));
  // The policy itself is full of single quotes ('self'), so match the
  // attribute's own quote character.
  const content = tag && /\bcontent\s*=\s*(["'])(.*?)\1/is.exec(tag);
  if (!content) throw new Error('ui/dist/index.html has no Content-Security-Policy meta tag; rebuild the UI');
  // Vite writes the quotes as entities (&#39;self&#39;).
  const policy = content[2]
    .replace(/&#39;|&#x27;|&apos;/gi, "'")
    .replace(/&quot;|&#34;/gi, '"')
    .replace(/&amp;/gi, '&')
    .trim();
  if (/unsafe-eval|unsafe-inline|https?:|\*/.test(policy)) throw new Error(`refusing a loose CSP: ${policy}`);
  return `${policy}; frame-ancestors 'none'`;
}

/** Response headers for every file served from app://. */
function responseHeaders(csp, mime) {
  return {
    'content-type': mime,
    'content-security-policy': csp,
    'x-content-type-options': 'nosniff',
    'cross-origin-opener-policy': 'same-origin',
    'cross-origin-resource-policy': 'same-origin',
    'referrer-policy': 'no-referrer',
  };
}

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.woff2': 'font/woff2',
  '.ico': 'image/x-icon',
};
const mimeOf = (file) => MIME[path.extname(file).toLowerCase()] || 'application/octet-stream';

/**
 * Maps an app:// URL to a file under `root`, or null. Only our host, only
 * files inside root (no `..`, no encoded separators, no absolute paths).
 */
function resolveAppPath(root, url) {
  let u;
  try {
    u = new URL(url);
  } catch {
    return null;
  }
  if (u.protocol !== `${SCHEME}:` || u.host !== HOST) return null;
  let rel;
  try {
    rel = decodeURIComponent(u.pathname);
  } catch {
    return null;
  }
  if (rel.includes('\0') || rel.includes('\\')) return null;
  if (rel === '/' || rel === '') rel = '/index.html';
  const full = path.resolve(root, `.${rel}`);
  const base = path.resolve(root);
  if (full !== base && !full.startsWith(base + path.sep)) return null;
  return full;
}

/** In-app navigation stays on our origin; everything else is refused. */
function isInternalUrl(url) {
  try {
    const u = new URL(url);
    return u.protocol === `${SCHEME}:` && u.host === HOST;
  } catch {
    return false;
  }
}

/** Links that may be offered to the OS browser (after the user confirms): https only. */
function isExternalWebUrl(url) {
  try {
    const u = new URL(url);
    return u.protocol === 'https:' && !!u.hostname && !u.username && !u.password;
  } catch {
    return false;
  }
}

/**
 * Permissions the renderer may get without conditions. None: everything else
 * is refused, and the one exception is the microphone below.
 */
const ALLOWED_PERMISSIONS = Object.freeze(new Set());

/** WebRTC IP handling for every webContents (main.js lockDown). */
const WEBRTC_IP_POLICY = 'default_public_and_private_interfaces';

/** True only for a non-empty list that holds 'audio' and nothing else. */
function isAudioOnly(mediaTypes) {
  return Array.isArray(mediaTypes) && mediaTypes.length > 0 && mediaTypes.every((t) => t === 'audio');
}

/**
 * Voice v1: the microphone, and only that. A request (Electron's
 * setPermissionRequestHandler) passes only for permission 'media' asking for
 * audio alone, from our own page's top frame: the asking page, the requesting
 * URL and (when given) the security origin must all be app://mvs. Camera,
 * screen/display capture, a remote origin, a subframe or any other
 * permission: refused. `pageUrl` is the URL of the asking webContents.
 * V-C2: and only while we are in a call (`inCall`, from createCallGate), so
 * script in our page can't record without joining one.
 */
function allowPermissionRequest(pageUrl, permission, details, inCall) {
  if (ALLOWED_PERMISSIONS.has(permission)) return true;
  if (permission !== 'media' || !details || inCall !== true) return false;
  if (!isInternalUrl(pageUrl) || details.isMainFrame !== true) return false;
  if (!isInternalUrl(details.requestingUrl)) return false;
  if (details.securityOrigin !== undefined && !isInternalUrl(details.securityOrigin)) return false;
  return isAudioOnly(details.mediaTypes);
}

/**
 * The matching check (setPermissionCheckHandler): 'media' with mediaType
 * 'audio', from our own origin, top frame. Anything else: false. `pageUrl`
 * is the URL of the asking webContents, or undefined when Electron gives none.
 * Also only while `inCall` is true.
 */
function allowPermissionCheck(pageUrl, permission, requestingOrigin, details, inCall) {
  if (ALLOWED_PERMISSIONS.has(permission)) return true;
  if (permission !== 'media' || !details || inCall !== true) return false;
  if (pageUrl !== undefined && !isInternalUrl(pageUrl)) return false;
  if (!isInternalUrl(requestingOrigin) || details.isMainFrame !== true) return false;
  if (details.securityOrigin !== undefined && !isInternalUrl(details.securityOrigin)) return false;
  if (details.requestingUrl !== undefined && !isInternalUrl(details.requestingUrl)) return false;
  return details.mediaType === 'audio';
}

const hasParticipants = (state) => !!state && Array.isArray(state.participants) && state.participants.length > 0;

/**
 * V-C2: whether we are in a call, as main sees it from the core's answers and
 * events. It opens only on a successful joinCall (a CallState with
 * participants) and fails closed: it shuts on leaveCall (when sent), on any
 * joinCall that fails, on a call.state with no participants (left, ended,
 * timed out, refused), whenever the core is not up (a restarted core has no
 * call), and on quit. A joinCall answer that arrives after a leave, a core
 * restart or quit (an older generation) can't reopen it.
 */
function createCallGate() {
  let inCall = false;
  let gen = 0;
  const shut = () => {
    inCall = false;
    gen += 1;
  };
  return {
    get inCall() {
      return inCall;
    },
    /** Before a command goes to the core; returns a token for its answer. */
    beforeCommand(cmd) {
      if (cmd === 'leaveCall') shut();
      return gen;
    },
    /** The core's answer ({ ok } or { error }) to a command sent with `token`. */
    afterCommand(cmd, token, res) {
      if (cmd !== 'joinCall') return;
      // joinCall leaves any other call first, so a failed one leaves us out.
      if (!res || res.error || !hasParticipants(res.ok)) return void shut();
      if (token === gen) inCall = true;
    },
    /** Every core event. */
    onEvent(name, payload) {
      // Only shuts: a join that is still being answered must not lose its
      // generation to the leave of the call it replaces.
      if (name === 'call.state' && !hasParticipants(payload)) inCall = false;
    },
    /** Every core status ({ state: 'starting' | 'up' | 'down', ... }). */
    onCoreStatus(status) {
      if (!status || status.state !== 'up') shut();
    },
    /** Quit. */
    end: shut,
  };
}

module.exports = {
  SCHEME,
  HOST,
  ORIGIN,
  WEB_PREFERENCES,
  ALLOWED_PERMISSIONS,
  WEBRTC_IP_POLICY,
  allowPermissionRequest,
  allowPermissionCheck,
  createCallGate,
  cspFromIndexHtml,
  responseHeaders,
  mimeOf,
  resolveAppPath,
  isInternalUrl,
  isExternalWebUrl,
};
