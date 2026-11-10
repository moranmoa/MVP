'use strict';

// Sign out on this computer (#304), the desktop's part.
// - Without the recovery words, the account is lost for good. The page's own
//   confirmation could be clicked by a script injected into it, so main asks
//   again in a native dialog the page can't reach. Declining answers
//   `cancelled`, and the core is never asked.
// - After the core says ok: what the page stored (localStorage, IndexedDB,
//   caches for app://mvs) and the app's log go too. Failures there are
//   ignored: the account is already gone, and they hold no key material.

const fs = require('fs');
const path = require('path');

/**
 * @param {object} o
 * @param {{ words?: string[], withoutWords?: boolean }} o.args the request's named args
 * @param {() => Promise<boolean>} o.confirmWithoutWords the native dialog; true = go ahead
 * @param {() => Promise<object>} o.request sends signOut to the core
 * @param {() => Promise<void>} o.clearStorage clears the page's storage
 * @param {() => void} o.clearLogs deletes the app's log files
 */
async function signOut({ args, confirmWithoutWords, request, clearStorage, clearLogs }) {
  if (args && args.withoutWords === true && !(await confirmWithoutWords())) return { error: { code: 'cancelled' } };
  const res = await request();
  if (res && !res.error) {
    await clearStorage().catch(() => {});
    try {
      clearLogs();
    } catch {
      // nothing to do
    }
  }
  return res;
}

/** Deletes mvs.log and its rotated copy in `dir`. */
function clearLogFiles(dir) {
  for (const name of ['mvs.log', 'mvs.log.1']) {
    try {
      fs.rmSync(path.join(dir, name), { force: true });
    } catch {
      // a locked file stays; it's truncated on the next rotation
    }
  }
}

/** The native dialog's text, in both languages (main has no catalogue). */
const WITHOUT_WORDS_DIALOG = {
  type: 'warning',
  buttons: ['ביטול / Cancel', 'יציאה בלי המילים / Sign out without the words'],
  defaultId: 0,
  cancelId: 0,
  noLink: true,
  message: 'לצאת בלי מילות השחזור? / Sign out without the recovery words?',
  detail:
    'החשבון יאבד לתמיד: לא יהיה אפשר להיכנס אליו שוב בשום מחשב.\n' +
    "The account will be lost for good: you can't sign in to it again on any computer.",
};

module.exports = { signOut, clearLogFiles, WITHOUT_WORDS_DIALOG };
