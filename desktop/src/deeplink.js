// mvs:// links that reach the app from the OS (a click in a browser or chat).
// Anything the OS hands us on the command line is untrusted: only an invite
// link of the exact core format passes, and it is used for one thing only,
// as the argument of joinServer (core/api model.rs `parse_invite_link`).

'use strict';

const SCHEME = 'mvs';
/** mvs://join/<server id, 32 bytes hex>/<invite record id, 32 bytes hex> */
const INVITE = /^mvs:\/\/join\/([0-9a-f]{64})\/([0-9a-f]{64})\/?$/;
const MAX_LEN = 200;

/** The canonical invite link, or null. */
function parseInviteLink(raw) {
  if (typeof raw !== 'string' || raw.length > MAX_LEN) return null;
  const m = INVITE.exec(raw.trim().toLowerCase());
  return m ? `mvs://join/${m[1]}/${m[2]}` : null;
}

/**
 * The invite link in a command line (first launch or a second instance on
 * Windows and Linux), or null. Only arguments that start with mvs:// count.
 */
function inviteFromArgv(argv) {
  if (!Array.isArray(argv)) return null;
  for (const a of argv) {
    if (typeof a === 'string' && a.toLowerCase().startsWith(`${SCHEME}://`)) return parseInviteLink(a);
  }
  return null;
}

module.exports = { SCHEME, parseInviteLink, inviteFromArgv };
