// Build-time settings for the core, kept out of git: the release workflow
// writes build/mvs-config.json (from a repository variable) and packaging
// puts it next to the app as resources/mvs-config.json. A checkout uses the
// MVS_RELAY environment variable instead.
//
//   { "relay": "/dns4/<host>/tcp/443/wss/p2p/<peer id>" }
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

module.exports = { MAX_RELAY, validRelay, relayFor };
