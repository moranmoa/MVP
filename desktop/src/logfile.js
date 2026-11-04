// The shell's log file, for the "core unavailable" screen's "open logs"
// (T8-01). It holds only what main logs anyway: its own short notes and the
// core's stderr (counts and error kinds; never words, keys or message text,
// see core/api README). Capped: at 1 MiB the file moves to mvs.log.1.

'use strict';

const fs = require('node:fs');
const path = require('node:path');

const MAX_BYTES = 1024 * 1024;

/** Nothing that says where someone is or who they are reaches the file. */
function redact(text) {
  return text
    .replace(/\b[0-9a-f]{64}\b/gi, '<id>')
    .replace(/\b(?:12D3KooW|Qm)[1-9A-HJ-NP-Za-km-z]{40,60}\b/g, '<peer>')
    .replace(/\/(dns|dns4|dns6|dnsaddr)\/[^/\s]+/g, '/$1/<host>')
    .replace(/\b(?:\d{1,3}\.){3}\d{1,3}\b/g, '<ip>')
    // IPv6: the full eight-group form, and any compressed form with "::"
    // (not plain "12:30:45" clock times).
    .replace(/\b(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}\b/gi, '<ip>')
    .replace(/(?:\b[0-9a-f]{1,4}(?::[0-9a-f]{1,4}){0,6})?::(?:[0-9a-f]{1,4}(?::[0-9a-f]{1,4}){0,6}\b)?/gi, '<ip>');
}

function createLog(dir, { maxBytes = MAX_BYTES } = {}) {
  const file = path.join(dir, 'mvs.log');
  try {
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  } catch {
    // Logging must never stop the app.
  }
  // Literal names to hide wherever they appear (the configured relay host:
  // a bare "relay.example.org:443" in an error text has no /dns*/ around it).
  const literals = [];
  function log(line) {
    let clean = redact(String(line).replace(/[\r\n]+/g, ' '));
    for (const re of literals) clean = clean.replace(re, '<host>');
    writeLine(clean);
  }
  log.hide = (name) => {
    if (typeof name === 'string' && name.length >= 3) literals.push(new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi'));
  };
  return log;

  function writeLine(clean) {
    try {
      const text = `${new Date().toISOString()} ${clean.slice(0, 2000)}\n`;
      if (fs.existsSync(file) && fs.statSync(file).size + text.length > maxBytes) fs.renameSync(file, `${file}.1`);
      fs.appendFileSync(file, text, { mode: 0o600 });
    } catch {
      // ignore: a full disk or a locked file is no reason to crash
    }
  }
}

module.exports = { createLog, redact, MAX_BYTES };
