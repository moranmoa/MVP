// Build-time settings for the core, kept out of git: the release workflow
// writes build/mvs-config.json (from a repository variable) and packaging
// puts it next to the app as resources/mvs-config.json. A checkout uses the
// MVS_RELAY (and MVS_BOOTSTRAP) environment variables instead.
//
//   { "relay": "/dns4/<host>/tcp/443/wss/p2p/<peer id>",
//     "bootstrap": ["/dns4/<host>/tcp/4001/p2p/<peer id>",        (optional)
//                   "/dns4/<host>/udp/4001/quic-v1/p2p/<peer id>"],
//     "stun": "stun:<host>:3478" }                            (optional, voice)
//
// Anything malformed is ignored with a warning: the app then runs without a
// relay (offline) rather than handing the core something unchecked.

'use strict';

const fs = require('node:fs');
const path = require('node:path');

const MAX_RELAY = 512;

/** The relay's peer ID must be pinned (#114): the address ends in /p2p/<base58 ID>. */
const PEER_SUFFIX = /\/p2p\/[1-9A-HJ-NP-Za-km-z]{46,60}$/;

/** A multiaddr-shaped string: starts with '/', no whitespace, bounded, ends in /p2p/<id>. */
function validRelay(value) {
  return (
    typeof value === 'string' &&
    value.length <= MAX_RELAY &&
    value.startsWith('/') &&
    !/\s/.test(value) &&
    PEER_SUFFIX.test(value)
  );
}

/**
 * The relay to give the core, or null.
 * @param {{ packaged: boolean, resourcesPath?: string, env?: object, warn?: (m: string) => void }} o
 */
function relayFor({ packaged, resourcesPath, env = process.env, warn = (m) => console.warn(m) }) {
  let value;
  if (packaged) {
    const file = path.join(resourcesPath, 'mvs-config.json');
    if (!fs.existsSync(file)) return null;
    try {
      value = JSON.parse(fs.readFileSync(file, 'utf8')).relay;
    } catch {
      warn('mvs-config.json is not valid JSON; no relay');
      return null;
    }
  } else {
    value = env.MVS_RELAY;
  }
  if (value === undefined || value === null || value === '') return null;
  if (!validRelay(value)) {
    warn('ignoring a malformed relay address; no relay');
    return null;
  }
  return value;
}


/** Most `bootstrap` entries passed on (mvs-core takes at most 4). */
const MAX_BOOTSTRAP = 4;

/**
 * The relay's direct addresses to give the core as `--bootstrap` (#219): the
 * `bootstrap` list of mvs-config.json, or MVS_BOOTSTRAP (comma-separated) in
 * a checkout. List the relay's QUIC address (`/udp/4001/quic-v1`) too: only a
 * QUIC connection to it shows us our public UDP address, which hole punching
 * over QUIC (#243) needs. Each must pass the same check as the relay; bad
 * entries are dropped with a warning, and a list that isn't one is ignored.
 * @param {{ packaged: boolean, resourcesPath?: string, env?: object, warn?: (m: string) => void }} o
 * @returns {string[]}
 */
function bootstrapFor({ packaged, resourcesPath, env = process.env, warn = (m) => console.warn(m) }) {
  let list;
  if (packaged) {
    const file = path.join(resourcesPath, 'mvs-config.json');
    if (!fs.existsSync(file)) return [];
    try {
      list = JSON.parse(fs.readFileSync(file, 'utf8')).bootstrap;
    } catch {
      return []; // relayFor already warned
    }
  } else {
    list = env.MVS_BOOTSTRAP ? env.MVS_BOOTSTRAP.split(',') : undefined;
  }
  if (list === undefined || list === null) return [];
  if (!Array.isArray(list)) {
    warn('ignoring a malformed bootstrap list');
    return [];
  }
  const ok = list.filter(validRelay);
  if (ok.length < list.length) warn('ignoring malformed bootstrap addresses');
  if (ok.length > MAX_BOOTSTRAP) warn(`using the first ${MAX_BOOTSTRAP} bootstrap addresses`);
  return ok.slice(0, MAX_BOOTSTRAP);
}

/** Longest STUN URL passed on. */
const MAX_STUN = 256;

/**
 * A plain STUN URL, the same rule as the UI's iceServersFor (ui/src/voice/
 * sdp.ts): `stun:` or `stuns:`, then a host (and port) with no whitespace,
 * credentials (@), path, query or fragment. TURN or anything else: no.
 */
function validStun(value) {
  return typeof value === 'string' && value.length <= MAX_STUN && /^stuns?:[^\s@/?#]+$/i.test(value.trim());
}

/**
 * Our own STUN server for voice (#222), for the page's WebRTC: `stun` in
 * mvs-config.json, or MVS_STUN in a checkout. Null when there is none (calls
 * then work on the same network only); a malformed one is ignored with a
 * warning that never names it.
 * @param {{ packaged: boolean, resourcesPath?: string, env?: object, warn?: (m: string) => void }} o
 * @returns {string | null}
 */
function stunFor({ packaged, resourcesPath, env = process.env, warn = (m) => console.warn(m) }) {
  let value;
  if (packaged) {
    const file = path.join(resourcesPath, 'mvs-config.json');
    if (!fs.existsSync(file)) return null;
    try {
      value = JSON.parse(fs.readFileSync(file, 'utf8')).stun;
    } catch {
      return null; // relayFor already warned
    }
  } else {
    value = env.MVS_STUN;
  }
  if (value === undefined || value === null || value === '') return null;
  if (!validStun(value)) {
    warn('ignoring a malformed STUN URL; no STUN');
    return null;
  }
  return value.trim();
}

/** The host of a STUN URL (for the file log to hide), or null. */
function stunHost(url) {
  const m = typeof url === 'string' && /^stuns?:(?:\[([^\]]+)\]|([^:]+))/i.exec(url);
  return m ? m[1] || m[2] : null;
}

/**
 * The host of a relay or bootstrap multiaddr (`/dns4/<name>` and the other
 * DNS forms, `/ip4/<addr>` or `/ip6/<addr>`), to hide from the file log;
 * null for anything else.
 */
function logHost(addr) {
  const m = typeof addr === 'string' && /^\/(?:dns(?:4|6|addr)?|ip4|ip6)\/([^/]+)/.exec(addr);
  return m ? m[1] : null;
}

module.exports = { MAX_RELAY, MAX_BOOTSTRAP, MAX_STUN, validRelay, relayFor, bootstrapFor, logHost, validStun, stunFor, stunHost };
