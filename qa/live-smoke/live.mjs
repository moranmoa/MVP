// Live release smoke, one side (ROLE=a owner, ROLE=b member): a release's
// mvs-core against the LIVE relay from that release's mvs-config.json.
// A creates a fresh server each round; B joins it by invite and sends; A
// answers. Per round: time to join, and time to first message each way.
//
// The two containers sit on separate Docker networks, so they meet only
// through the relay (run.sh). The relay address is never printed here.

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const ROLE = process.env.ROLE;
const OTHER = ROLE === 'a' ? 'b' : 'a';
const ROUNDS = Number(process.env.ROUNDS ?? 10);
const cfg = JSON.parse(readFileSync('/cfg/mvs-config.json', 'utf8'));
const RV = '/rendezvous';
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

const args = ['--data-dir', mkdtempSync(join(tmpdir(), `mvs-live-${ROLE}-`)), '--relay', cfg.relay];
for (const b of (cfg.bootstrap ?? []).slice(0, 4)) args.push('--bootstrap', b);
// JOIN_LOG=1: the core's join steps and sync's debug lines (the desktop's
// MVS_DEBUG_LOG filter) go to stdout as "core: ...", and run.sh masks them.
// Otherwise stderr is dropped: the core logs addresses.
const JOIN_LOG = process.env.JOIN_LOG === '1';
const child = spawn('/core/mvs-core', args, {
  stdio: ['pipe', 'pipe', 'pipe'],
  env: { ...process.env, ...(JOIN_LOG ? { RUST_LOG: 'off,mvs::join=info,sync=debug' } : {}) },
});
if (JOIN_LOG) createInterface({ input: child.stderr }).on('line', (l) => console.log(`core: ${l}`));
else child.stderr.on('data', () => {});
const waiting = new Map();
let next = 1;
createInterface({ input: child.stdout }).on('line', (line) => {
  let m;
  try {
    m = JSON.parse(line);
  } catch {
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
async function until(fn, timeout) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const v = await fn().catch(() => null);
    if (v) return v;
    await sleep(200);
  }
  return null;
}
const general = async (serverId) => (await call('listChannels', { serverId }))[0].id;
const bodies = async (channelId) => {
  const n = await call('getMessageCount', { channelId });
  return n ? (await call('getMessages', { channelId, start: 0, end: n })).messages.map((m) => m.body) : [];
};

const rows = [];
try {
  console.log(`${ROLE}: core ${(await call('getAppInfo')).version}`);
  await call('createIdentity', { input: { locale: 'en', country: 'IL', birthDate: '1990-01-01' } });
  const words = await call('getRecoveryWords');
  const ch = await call('getRecoveryChallenge').catch(() => null);
  const pos = ch?.positions ?? [1, 5, 9];
  await call('checkRecoveryWords', { answers: Object.fromEntries(pos.map((p) => [p, words[p - 1]])) });
  await call('updateProfile', { input: { displayName: `live-${ROLE}` } });

  for (let r = 1; r <= ROUNDS; r++) {
    if (ROLE === 'a') {
      const s = await call('createServer', { input: { name: `live smoke ${r}`, ageRestriction: 'none' } });
      const chan = await general(s.id);
      put(`invite-${r}`, { link: s.inviteLink });
      const { at, text } = await take(`sent-${r}`);
      const got = await until(async () => (await bodies(chan)).includes(text), 60_000);
      const bToA = got ? Date.now() - at : null;
      const reply = `reply ${r} ${Date.now()}`;
      const sentAt = Date.now();
      await call('sendMessage', { channelId: chan, body: reply });
      put(`reply-${r}`, { at: sentAt, text: reply });
      const { aToB } = await take(`got-${r}`);
      rows.push({ round: r, bToA, aToB });
      console.log(`round ${r}: B->A ${bToA ?? 'MISSING'} ms, A->B ${aToB ?? 'MISSING'} ms`);
    } else {
      const { link } = await take(`invite-${r}`);
      console.log(`round ${r}: join start ${new Date().toISOString()}`);
      const t0 = Date.now();
      await call('joinServer', { link });
      const s = await until(async () => (await call('listServers')).find((x) => x.name === `live smoke ${r}`), 120_000);
      const join = s ? Date.now() - t0 : null;
      console.log(`round ${r}: join ${join === null ? 'FAILED (120 s)' : `${(join / 1000).toFixed(1)} s`}`);
      if (!s) {
        put(`sent-${r}`, { at: Date.now(), text: '(no join)' });
        put(`got-${r}`, { aToB: null });
        rows.push({ round: r, join: null });
        continue;
      }
      const chan = await general(s.id);
      const text = `hello ${r} ${Date.now()}`;
      // The joiner may be refused (not-allowed) until its join is accepted: wait, and say how long.
      const t1 = Date.now();
      let refusals = 0;
      for (;;) {
        try {
          await call('sendMessage', { channelId: chan, body: text });
          break;
        } catch (e) {
          if (!/not-allowed/.test(e.message) || Date.now() - t1 > 60_000) throw e;
          refusals++;
          await sleep(500);
        }
      }
      // #285: the first send right after the join must succeed (no not-allowed).
      console.log(
        refusals
          ? `round ${r}: FAIL first send refused ${refusals}x (not-allowed); it worked ${((Date.now() - t1) / 1000).toFixed(1)} s after joining`
          : `round ${r}: first send OK (${Date.now() - t1} ms)`,
      );
      const at = Date.now();
      put(`sent-${r}`, { at, text });
      const reply = await take(`reply-${r}`);
      const got = await until(async () => (await bodies(chan)).includes(reply.text), 60_000);
      put(`got-${r}`, { aToB: got ? Date.now() - reply.at : null });
      rows.push({ round: r, join });
    }
  }
} catch (e) {
  console.log(`${ROLE}: FAIL ${e.message}`);
  put('abort');
}
writeFileSync(join(RV, `rows-${ROLE}.json`), JSON.stringify(rows));
child.stdin.end();
process.exit(0);
