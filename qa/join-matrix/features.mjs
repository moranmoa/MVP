// 0.4.0 features between two release cores (run.sh with SCRIPT=features.mjs).
// MODE=new: A (owner) and B (a member) check channel order (G-11), unread
// count and firstUnreadIndex then markRead, and a <@id> mention (mentionsMe,
// the channel's mention badge). MODE=backup: B restores A's words on a fresh
// device and must get A's servers back from the backup (/mvs/backup/1) with
// no invite, while A is online.

import { spawn } from 'node:child_process';
import { createWriteStream, existsSync, mkdtempSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const ROLE = process.env.ROLE;
const OTHER = ROLE === 'a' ? 'b' : 'a';
const MODE = process.env.MODE ?? 'new';
const RV = '/rendezvous';
const t0 = Date.now();
const log = (...m) => console.log(`[${ROLE} +${((Date.now() - t0) / 1000).toFixed(1)}s]`, ...m);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
const result = (ok, text, detail = '') => {
  if (!ok) failed++;
  console.log(`${ok ? 'PASS' : 'FAIL'} ${text}${detail ? ` (${detail})` : ''}`);
};
const put = (name, data = {}) => {
  const f = join(RV, `${ROLE}.${name}.json`);
  writeFileSync(`${f}.tmp`, JSON.stringify(data));
  renameSync(`${f}.tmp`, f);
};
async function take(name, timeout = 180_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const f = join(RV, `${OTHER}.${name}.json`);
    if (existsSync(f)) return JSON.parse(readFileSync(f, 'utf8'));
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${OTHER}'s ${name}`);
}
async function until(fn, timeout) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const v = await fn().catch(() => null);
    if (v) return v;
    await sleep(500);
  }
  return null;
}

const child = spawn('/core/mvs-core', ['--data-dir', mkdtempSync(join(tmpdir(), `mvs-${ROLE}-`)), '--relay', process.env.MVS_RELAY], {
  stdio: ['pipe', 'pipe', 'pipe'],
  env: { ...process.env, RUST_LOG: 'info,sync=debug,api=debug' },
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

async function newIdentity(name) {
  await call('createIdentity', { input: { locale: 'en', country: 'IL', birthDate: '1990-01-01' } });
  const words = await call('getRecoveryWords');
  const ch = await call('getRecoveryChallenge').catch(() => null);
  const pos = ch?.positions ?? [1, 5, 9];
  await call('checkRecoveryWords', { answers: Object.fromEntries(pos.map((p) => [p, words[p - 1]])) });
  await call('updateProfile', { input: { displayName: name } });
  return words;
}
const names = (list) => list.map((c) => c.name).join(',');

try {
  log(`core ${(await call('getAppInfo')).version}, mode ${MODE}`);
  if (ROLE === 'a') {
    const words = await newIdentity('feat-a');
    const s = await call('createServer', { input: { name: 'features', ageRestriction: 'none' } });
    const general = (await call('listChannels', { serverId: s.id }))[0];
    const c2 = await call('createChannel', { serverId: s.id, input: { name: 'two', adult: false } });
    const c3 = await call('createChannel', { serverId: s.id, input: { name: 'three', adult: false } });
    if (MODE === 'backup') {
      await call('createServer', { input: { name: 'features 2', ageRestriction: 'none' } });
      await sleep(5000); // let the backup be put
      put('ready', { words });
      const done = await take('done', 240_000).catch((e) => ({ error: e.message }));
      log('restored device reports:', JSON.stringify(done));
    } else {
      put('ready', { link: s.inviteLink, me: (await call('getIdentity')).id });
      const { bId } = await take('joined');
      // Channel order: move "three" before general.
      const order = await call('moveChannel', { serverId: s.id, channelId: c3.id, before: general.id });
      result(order[0]?.id === c3.id, 'owner: moveChannel puts "three" first', names(order));
      put('moved', { order: order.map((c) => c.id) });
      // Unread and a mention for B in "two".
      for (const body of ['one', 'two', `hey <@${bId}> look`]) await call('sendMessage', { channelId: c2.id, body });
      put('sent', { channel: c2.id });
      const done = await take('done', 240_000).catch((e) => ({ error: e.message }));
      log('member reports:', JSON.stringify(done));
    }
  } else if (MODE === 'backup') {
    const { words } = await take('ready', 300_000);
    const id = await call('restoreIdentity', { words });
    log(`restored ${id.id.slice(0, 8)}`);
    const start = Date.now();
    const got = await until(async () => {
      const list = await call('listServers');
      return list.length >= 2 ? list : null;
    }, 180_000);
    result(!!got, 'fresh-device restore rejoins the servers from the backup (no invite)', got ? `${got.map((s) => s.name).join(', ')} in ${((Date.now() - start) / 1000).toFixed(1)} s` : 'none after 180 s');
    put('done', { ok: !!got });
  } else {
    const { link } = await take('ready', 300_000);
    await newIdentity('feat-b');
    const me = (await call('getIdentity')).id;
    await call('joinServer', { link });
    const s = await until(async () => (await call('listServers')).find((x) => x.name === 'features'), 120_000);
    if (!s) throw new Error('join failed');
    put('joined', { bId: me });
    const { order } = await take('moved');
    const t1 = Date.now();
    const seen = await until(async () => {
      const list = await call('listChannels', { serverId: s.id });
      return list.map((c) => c.id).join() === order.join() ? list : null;
    }, 60_000);
    result(!!seen, 'member sees the owner\'s new channel order', seen ? `${names(seen)} in ${((Date.now() - t1) / 1000).toFixed(1)} s` : 'not within 60 s');
    const { channel } = await take('sent');
    const ch = await until(async () => {
      const c = (await call('listChannels', { serverId: s.id })).find((x) => x.id === channel);
      return c?.unreadCount >= 3 ? c : null;
    }, 60_000);
    result(ch?.unreadCount === 3, 'unreadCount = 3 after 3 new messages', JSON.stringify(ch && { unreadCount: ch.unreadCount, firstUnreadIndex: ch.firstUnreadIndex, mentions: ch.mentions }));
    const n = await call('getMessageCount', { channelId: channel });
    result(ch?.firstUnreadIndex === n - 3, 'firstUnreadIndex points at the first new message', `firstUnreadIndex ${ch?.firstUnreadIndex}, count ${n}`);
    const msgs = (await call('getMessages', { channelId: channel, start: 0, end: n })).messages;
    const mention = msgs.find((m) => m.body.includes('<@'));
    result(mention?.mentionsMe === true && msgs.filter((m) => m.mentionsMe).length === 1, 'the <@me> message has mentionsMe, the others not', `${msgs.filter((m) => m.mentionsMe).length} of ${msgs.length}`);
    result(ch?.mentions >= 1, 'the channel shows a mention badge', `mentions ${ch?.mentions}`);
    await call('markRead', { channelId: channel });
    const after = (await call('listChannels', { serverId: s.id })).find((x) => x.id === channel);
    result(!after.unreadCount && !after.mentions, 'markRead clears the unread count and the mention badge', JSON.stringify({ unreadCount: after.unreadCount, mentions: after.mentions, firstUnreadIndex: after.firstUnreadIndex }));
    put('done', { failed });
  }
} catch (e) {
  result(false, 'the check stopped', e.message);
  if (ROLE === 'b') put('done', { error: e.message });
}
await sleep(1000);
child.stdin.end();
process.exit(failed ? 1 : 0);
