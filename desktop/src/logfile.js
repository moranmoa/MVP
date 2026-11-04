// The shell's log file, for the "core unavailable" screen's "open logs"
// (T8-01). It holds only what main logs anyway: its own short notes and the
// core's stderr (counts and error kinds; never words, keys or message text,
// see core/api README). Capped: at 1 MiB the file moves to mvs.log.1.

'use strict';

const fs = require('node:fs');
const path = require('node:path');

const MAX_BYTES = 1024 * 1024;

function createLog(dir, { maxBytes = MAX_BYTES } = {}) {
  const file = path.join(dir, 'mvs.log');
  try {
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  } catch {
    // Logging must never stop the app.
  }
  return function log(line) {
    try {
      const text = `${new Date().toISOString()} ${String(line).replace(/[\r\n]+/g, ' ').slice(0, 2000)}\n`;
      if (fs.existsSync(file) && fs.statSync(file).size + text.length > maxBytes) fs.renameSync(file, `${file}.1`);
      fs.appendFileSync(file, text, { mode: 0o600 });
    } catch {
      // ignore: a full disk or a locked file is no reason to crash
    }
  };
}

module.exports = { createLog, MAX_BYTES };
