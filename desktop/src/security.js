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

/** Links that may be offered to the OS browser (after the user confirms). */
function isExternalWebUrl(url) {
  try {
    const u = new URL(url);
    return (u.protocol === 'https:' || u.protocol === 'http:') && !!u.hostname && !u.username && !u.password;
  } catch {
    return false;
  }
}

/**
 * Permissions the renderer may get. None for now: the mock UI needs none.
 * The microphone comes with voice (phase 2) and is decided then.
 */
const ALLOWED_PERMISSIONS = Object.freeze(new Set());

module.exports = {
  SCHEME,
  HOST,
  ORIGIN,
  WEB_PREFERENCES,
  ALLOWED_PERMISSIONS,
  cspFromIndexHtml,
  responseHeaders,
  mimeOf,
  resolveAppPath,
  isInternalUrl,
  isExternalWebUrl,
};
