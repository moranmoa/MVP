// One side of a cross-version join check (run.sh): ROLE=a is the owner,
// ROLE=b the joiner. MODE=new gives B its own identity; MODE=restore
// restores B from A's recovery words (the same identity joining again from
// a second computer). Each side runs its own release's mvs-core, with
// RUST_LOG on; stderr goes to /rendezvous/core-<role>.log (run.sh masks it).

import { spawn } from 'node:child_process';
import { createWriteStream, existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const ROLE = process.env.ROLE;
const OTHER = ROLE === 'a' ? 'b' : 'a';
const MODE = process.env.MODE ?? 'new';
const JOIN_TIMEOUT = Number(process.env.JOIN_TIMEOUT ?? 120) * 1000;
const RV = '/rendezvous';
const t0 = Date.now();
const log = (...m) => console.log(`[${ROLE} +${((Date.now() - t0) / 1000).toFixed(1)}s]`, ...m);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const put = (name, data = {}) => writeFileSync(join(RV, `${ROLE}.${name}.json`), JSON.stringify(data));
async function take(name, timeout = 300_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const f = join(RV, `${OTHER}.${name}.json`);
    if (existsSync(f)) return JSON.parse(readFileSync(f, 'utf8'));
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${OTHER}'s ${name}`);
}

const child = spawn('/core/mvs-core', ['--data-dir', mkdtempSync(join(tmpdir(), `mvs-${ROLE}-`)), '--relay', process.env.MVS_RELAY], {
  stdio: ['pipe', 'pipe', 'pipe'],
  env: { ...process.env, RUST_LOG: process.env.RUST_LOG ?? 'info,sync=debug,api=debug' },
});
child.stderr.pipe(createWriteStream(join(RV, `core-${ROLE}.log`)));
const waiting = new Map();
let next = 1;
createInterface({ input: child.stdout }).on('line', (line) => {
  let m;
  try {
    m = JSON.parse(line);
  } catch {
    return;
  }
  if (m.event) {
    const { link, ...rest } = m.payload ?? {};
    log(`event ${m.event}`, JSON.stringify(rest).slice(0, 160));
    return;
  }
  if (m.id === undefined || !waiting.has(m.id)) return;
  const { resolve, reject } = waiting.get(m.id);
  waiting.delete(m.id);
  m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.ok);
});
const call = (cmd, a = {}) =>
  new Promise((resolve, reject) => {
    const id = next++;
    waiting.set(id, { resolve, reject: (e) => reject(new Error(`${cmd}: ${e.message}`)) });
    child.stdin.write(`${JSON.stringify({ id, cmd, args: a })}\n`);
  });

let result = 'error';
try {
  log(`core ${(await call('getAppInfo')).version}, mode ${MODE}`);
  if (ROLE === 'a' || MODE === 'new') {
    await call('createIdentity', { input: { locale: 'en', country: 'IL', birthDate: '1990-01-01' } });
    const words = await call('getRecoveryWords');
    const ch = await call('getRecoveryChallenge').catch(() => null);
    const pos = ch?.positions ?? [1, 5, 9];
    await call('checkRecoveryWords', { answers: Object.fromEntries(pos.map((p) => [p, words[p - 1]])) });
    await call('updateProfile', { input: { displayName: `join-${ROLE}` } });
    if (ROLE === 'a') {
      const s = await call('createServer', { input: { name: 'join matrix', ageRestriction: 'none' } });
      const channel = (await call('listChannels', { serverId: s.id }))[0].id;
      await call('sendMessage', { channelId: channel, body: 'hello from the owner' });
      // The words cross only through the run's private rendezvous folder.
      put('ready', { link: s.inviteLink, words, ownerId: (await call('getIdentity')).id });
      log('server created, invite handed over');
      const done = await take('done', JOIN_TIMEOUT + 60_000).catch((e) => ({ error: e.message }));
      log('members now:', JSON.stringify((await call('listMembers', { serverId: s.id })).map((m) => m.displayName || m.id.slice(0, 8))));
      log('joiner reports:', JSON.stringify(done));
      result = 'owner-done';
    }
  }
  if (ROLE === 'b') {
    const { link, words, ownerId } = await take('ready');
    if (MODE === 'restore') {
      const id = await call('restoreIdentity', { words });
      log(`restored: same identity as the owner = ${id.id === ownerId}`);
      log('servers right after restore:', JSON.stringify((await call('listServers')).map((s) => s.name)));
    }
    const started = Date.now();
    log('joinServer ->', JSON.stringify(await call('joinServer', { link })));
    const end = started + JOIN_TIMEOUT;
    let server = null;
    while (Date.now() < end) {
      server = (await call('listServers')).find((s) => s.name === 'join matrix');
      if (server) break;
      await sleep(500);
    }
    if (server) {
      const s = ((Date.now() - started) / 1000).toFixed(1);
      const channel = (await call('listChannels', { serverId: server.id }))[0].id;
      let history = [];
      for (let i = 0; i < 20 && !history.length; i++) {
        const n = await call('getMessageCount', { channelId: channel });
        history = n ? (await call('getMessages', { channelId: channel, start: 0, end: n })).messages.map((m) => m.body) : [];
        if (!history.length) await sleep(500);
      }
      const sent = await call('sendMessage', { channelId: channel, body: 'hello from the joiner' }).then(
        () => 'ok',
        (e) => e.message,
      );
      result = `JOINED in ${s} s; owner's message ${history.includes('hello from the owner') ? 'received' : 'NOT received'}; send ${sent}`;
    } else {
      await call('cancelJoin', { link }).catch(() => {});
      result = `HANG: not joined after ${JOIN_TIMEOUT / 1000} s`;
    }
    log(result);
    put('done', { result });
  }
} catch (e) {
  result = `FAIL ${e.message}`;
  log(result);
  if (ROLE === 'b') put('done', { result });
}
await sleep(1000);
child.stdin.end();
process.exit(0);
