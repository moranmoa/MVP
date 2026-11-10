#!/usr/bin/env node
// A stand-in for mvs-core that speaks the same line protocol (core/api
// README, "Sidecar") and misbehaves on cue, for the shell's tests:
//   getAppInfo        -> answers with its argv and the env keys it got
//   joinServer        -> an event, then the response (events come first)
//   getRecoveryWords  -> words (the shell must never log them)
//   getStorageUsage   -> never answers (timeout)
//   clearCache        -> exits (crash)
//   exportDiagnostics -> a 17 MiB line (over the shell's cap)
//   joinCall          -> call.state, then the CallState (me alone); channel
//                        'e'x64 is ended by the core 300 ms later
//   leaveCall         -> call.state with no participants, then null
'use strict';

const WORDS = ['fake', 'words', 'never', 'logged'];
let buf = '';
const out = (o) => process.stdout.write(`${JSON.stringify(o)}\n`);
process.stderr.write('fake core up\n');

// Every command received is appended to <data-dir>/commands.log, so tests can
// see what main sent on its own (e.g. joinServer from an mvs:// link).
const fs = require('node:fs');
const path = require('node:path');
const dataDir = process.argv[process.argv.indexOf('--data-dir') + 1];
const record = (m) => {
  try {
    fs.mkdirSync(dataDir, { recursive: true });
    fs.appendFileSync(path.join(dataDir, 'commands.log'), `${JSON.stringify({ cmd: m.cmd, args: m.args })}\n`);
  } catch {
    // no data dir in some unit tests
  }
};

process.stdin.on('data', (chunk) => {
  buf += chunk;
  let nl;
  while ((nl = buf.indexOf('\n')) !== -1) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    let m;
    try {
      m = JSON.parse(line);
    } catch {
      out({ type: 'response', id: null, error: { code: 'bad-request' } });
      continue;
    }
    record(m);
    switch (m.cmd) {
      case 'getAppInfo':
        out({ type: 'response', id: m.id, ok: { argv: process.argv.slice(2), env: Object.keys(process.env).sort(), args: m.args } });
        break;
      case 'joinServer':
        out({ type: 'event', event: 'join.status', payload: { link: m.args.link, state: 'searching' } });
        out({ type: 'response', id: m.id, ok: { link: m.args.link, state: 'searching' } });
        break;
      case 'getRecoveryWords':
        out({ type: 'response', id: m.id, ok: WORDS });
        break;
      case 'getStorageUsage':
        break;
      case 'clearCache':
        process.exit(3);
        break;
      case 'exportDiagnostics':
        process.stdout.write(`{"type":"response","id":${m.id},"ok":"${'x'.repeat(17 * 1024 * 1024)}"}\n`);
        break;
      case 'joinCall': {
        const call = { channelId: m.args.channelId, participants: [{ identity: 'a'.repeat(64), muted: false }], full: false };
        out({ type: 'event', event: 'call.state', payload: call });
        out({ type: 'response', id: m.id, ok: call });
        if (m.args.channelId === 'e'.repeat(64)) {
          setTimeout(() => out({ type: 'event', event: 'call.state', payload: { ...call, participants: [] } }), 300);
        }
        break;
      }
      case 'leaveCall':
        out({ type: 'event', event: 'call.state', payload: { channelId: 'c'.repeat(64), participants: [], full: false } });
        out({ type: 'response', id: m.id, ok: null });
        break;
      case 'checkRecoveryWords':
        out({ type: 'response', id: m.id, error: { code: 'invalid-words', positions: [2, 'x', 5], secret: 'dropped' } });
        break;
      default:
        out({ type: 'response', id: m.id, ok: m.args ?? null });
    }
  }
});
process.stdin.on('end', () => process.exit(0));
