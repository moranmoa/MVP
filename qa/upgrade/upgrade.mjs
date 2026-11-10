// Profile upgrade check: a profile written by an older mvs-core must open in
// a newer one, with nothing lost. Runs inside Docker (run.sh), in one session
// D-Bus with one test keyring, so both cores see the same keychain entries.
//
//   node upgrade.mjs <old mvs-core> <new mvs-core>
//
// Old core: identity, word check, profile, server, a second channel, three
// messages. New core, same data dir: everything reads back the same, a new
// message can be sent, and a second start of the new core still works.

import { spawn } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const [OLD, NEW] = process.argv.slice(2);
const DATA = mkdtempSync(join(tmpdir(), 'mvs-upgrade-'));
let failed = 0;
const result = (ok, text, detail = '') => {
  if (!ok) failed++;
  console.log(`${ok ? 'PASS' : 'FAIL'} ${text}${detail ? ` (${detail})` : ''}`);
};

/** One core process; call(cmd, args) resolves with `ok` or throws the error. */
function core(exe) {
  const child = spawn(exe, ['--data-dir', DATA], { stdio: ['pipe', 'pipe', 'inherit'] });
  const waiting = new Map();
  let next = 1;
  createInterface({ input: child.stdout }).on('line', (line) => {
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (msg.id === undefined || !waiting.has(msg.id)) return; // events
    const { resolve, reject } = waiting.get(msg.id);
    waiting.delete(msg.id);
    if (msg.error) reject(new Error(JSON.stringify(msg.error)));
    else resolve(msg.ok);
  });
  const exited = new Promise((r) => child.on('exit', r));
  return {
    call(cmd, args = {}) {
      const id = next++;
      child.stdin.write(`${JSON.stringify({ id, cmd, args })}\n`);
      return new Promise((resolve, reject) => {
        waiting.set(id, { resolve, reject });
        setTimeout(() => waiting.has(id) && (waiting.delete(id), reject(new Error(`${cmd}: no answer in 120 s`))), 120_000);
      });
    },
    async close() {
      child.stdin.end();
      return exited;
    },
  };
}

async function snapshot(c) {
  const identity = await c.call('getIdentity');
  const servers = await c.call('listServers');
  const out = { identity: identity && { id: identity.id, displayName: identity.displayName }, servers: [] };
  for (const s of servers) {
    const channels = await c.call('listChannels', { serverId: s.id });
    const chans = [];
    for (const ch of channels) {
      const n = await c.call('getMessageCount', { channelId: ch.id });
      const range = n ? await c.call('getMessages', { channelId: ch.id, start: 0, end: n }) : null;
      const msgs = (range?.messages ?? range ?? []).map((m) => m.body);
      chans.push({ id: ch.id, name: ch.name, count: n, bodies: msgs });
    }
    out.servers.push({ id: s.id, name: s.name, inviteLink: s.inviteLink, channels: chans });
  }
  return out;
}

try {
  // ---- the old core writes a profile ----
  const old = core(OLD);
  console.log('old core:', JSON.stringify((await old.call('getAppInfo').catch(() => null))?.version ?? 'unknown'));
  await old.call('createIdentity', { input: { locale: 'he', country: 'IL', birthDate: '1990-01-01' } });
  const words = await old.call('getRecoveryWords');
  const challenge = await old.call('getRecoveryChallenge').catch(() => null);
  const positions = challenge?.positions ?? [1, 5, 9];
  await old.call('checkRecoveryWords', { answers: Object.fromEntries(positions.map((p) => [p, words[p - 1]])) });
  await old.call('updateProfile', { input: { displayName: 'Upgrade QA' } });
  const server = await old.call('createServer', { input: { name: 'upgrade check', ageRestriction: 'none' } });
  const channels = await old.call('listChannels', { serverId: server.id });
  const extra = await old.call('createChannel', { serverId: server.id, input: { name: 'second', adult: false } }).catch(() => null);
  for (const body of ['first message', 'שניה 👋', 'third, with "quotes"']) {
    await old.call('sendMessage', { channelId: channels[0].id, body });
  }
  if (extra) await old.call('sendMessage', { channelId: extra.id, body: 'in the second channel' });
  const before = await snapshot(old);
  await old.close();
  console.log('written by the old core:', JSON.stringify(before));

  // ---- the new core opens it ----
  const fresh = core(NEW);
  console.log('new core:', JSON.stringify((await fresh.call('getAppInfo').catch(() => null))?.version ?? 'unknown'));
  const after = await snapshot(fresh);
  result(after.identity?.id === before.identity?.id, 'same identity', after.identity?.id ?? 'none');
  result(after.identity?.displayName === before.identity?.displayName, 'same display name', after.identity?.displayName);
  result(JSON.stringify(after.servers.map((s) => [s.id, s.name, s.inviteLink])) === JSON.stringify(before.servers.map((s) => [s.id, s.name, s.inviteLink])), 'same servers and invite links');
  result(
    JSON.stringify(after.servers.map((s) => s.channels.map((c) => [c.id, c.name]))) === JSON.stringify(before.servers.map((s) => s.channels.map((c) => [c.id, c.name]))),
    'same channels',
  );
  result(
    JSON.stringify(after.servers.map((s) => s.channels.map((c) => c.bodies))) === JSON.stringify(before.servers.map((s) => s.channels.map((c) => c.bodies))),
    'same messages, in order',
    `${after.servers.flatMap((s) => s.channels).reduce((n, c) => n + c.count, 0)} messages`,
  );
  const sent = await fresh.call('sendMessage', { channelId: channels[0].id, body: 'after the upgrade' }).then(
    () => true,
    (e) => e.message,
  );
  result(sent === true, 'the new core can send in the old server', sent === true ? '' : sent);
  const nowWords = await fresh.call('getRecoveryWords').then(
    () => 'readable',
    (e) => e.message,
  );
  result(/not-allowed/.test(nowWords), 'the word check stays passed (words not readable again)', nowWords);
  await fresh.close();

  // ---- a second start of the new core (the migration runs once) ----
  const again = core(NEW);
  const third = await snapshot(again);
  await again.close();
  const count = (s) => s.servers.flatMap((x) => x.channels).reduce((n, c) => n + c.count, 0);
  result(third.identity?.id === before.identity?.id && count(third) === count(before) + 1, 'a second start of the new core opens it too', `${count(third)} messages`);
} catch (e) {
  result(false, 'the check stopped', e.message);
}
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
