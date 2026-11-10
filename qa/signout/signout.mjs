// Sign-out check (#304/#305), in Docker with a throwaway identity and a test
// keyring (run.sh): sign out with the words, then nothing of the account is
// left on this computer, and the next start is onboarding.
//
//   node signout.mjs <mvs-core>
//
// Checked: core.db, core.db-wal, core.db-shm and sync/ are gone from the data
// dir; the keychain holds no entry it held before; a restart has no identity.

import { execSync, spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const [CORE] = process.argv.slice(2);
const DATA = mkdtempSync(join(tmpdir(), 'mvs-signout-'));
let failed = 0;
const result = (ok, text, detail = '') => {
  if (!ok) failed++;
  console.log(`${ok ? 'PASS' : 'FAIL'} ${text}${detail ? ` (${detail})` : ''}`);
};

function core() {
  const child = spawn(CORE, ['--data-dir', DATA], { stdio: ['pipe', 'pipe', 'ignore'] });
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
  const exited = new Promise((r) => child.on('exit', r));
  return {
    call: (cmd, args = {}) =>
      new Promise((resolve, reject) => {
        const id = next++;
        waiting.set(id, { resolve, reject: (e) => reject(new Error(`${cmd}: ${e.message}`)) });
        child.stdin.write(`${JSON.stringify({ id, cmd, args })}\n`);
      }),
    close: () => (child.stdin.end(), exited),
  };
}

/** The identity's items in the test keyring (core/identity KEYCHAIN_SERVICE), labels and attributes only. */
function keychain() {
  try {
    return execSync('secret-tool search --all --unlock service io.github.moranmoa.mvs.identity.v1 2>/dev/null', { encoding: 'utf8' })
      .split('\n')
      .filter((l) => /^(label|attribute\.)/.test(l.trim()))
      .map((l) => l.trim().replace(/^(secret|attribute\.[a-z_]*key[a-z_]*) = .*/i, '$1 = <hidden>'));
  } catch {
    return [];
  }
}
const files = () => (existsSync(DATA) ? readdirSync(DATA).sort() : []);
const db = () => {
  try {
    const s = statSync(join(DATA, 'core.db'));
    return `inode ${s.ino} size ${s.size}`;
  } catch {
    return 'none';
  }
};

try {
  const c = core();
  console.log('core:', (await c.call('getAppInfo')).version);
  await c.call('createIdentity', { input: { locale: 'en', country: 'IL', birthDate: '1990-01-01' } });
  const words = await c.call('getRecoveryWords');
  const ch = await c.call('getRecoveryChallenge').catch(() => null);
  const pos = ch?.positions ?? [1, 5, 9];
  await c.call('checkRecoveryWords', { answers: Object.fromEntries(pos.map((p) => [p, words[p - 1]])) });
  await c.call('updateProfile', { input: { displayName: 'Sign-out QA' } });
  const s = await c.call('createServer', { input: { name: 'sign-out check', ageRestriction: 'none' } });
  const channel = (await c.call('listChannels', { serverId: s.id }))[0].id;
  await c.call('sendMessage', { channelId: channel, body: 'before sign-out' });

  const keysBefore = keychain();
  const filesBefore = files();
  console.log(`core.db before: ${db()}`);
  result(filesBefore.includes('core.db'), 'before: core.db exists', filesBefore.join(' '));
  result(keysBefore.length > 0, 'before: the keychain holds the identity', `${keysBefore.filter((l) => l.startsWith('label')).length} item(s)`);

  // A wrong word list is refused and changes nothing.
  const wrong = [...words].reverse();
  const refused = await c.call('signOut', { words: wrong }).then(
    () => 'signed out',
    (e) => e.message,
  );
  result(refused !== 'signed out' && files().includes('core.db'), 'sign-out with the wrong words is refused, nothing deleted', refused);

  await c.call('signOut', { words });
  await c.close();

  const after = files();
  console.log(`core.db right after sign-out (core stopped): ${db()}`);
  for (const f of ['core.db', 'core.db-wal', 'core.db-shm', 'sync']) result(!after.includes(f), `after: no ${f}`, after.join(' ') || 'data dir empty');
  const keysAfter = keychain();
  const left = keysBefore.filter((k) => k.startsWith('label') && keysAfter.includes(k));
  result(left.length === 0, 'after: no keychain entry of this identity is left', left.join('; ') || `${keysAfter.filter((l) => l.startsWith('label')).length} item(s) in the keyring`);

  const again = core();
  const id = await again.call('getIdentity').catch((e) => `error ${e.message}`);
  result(id === null, 'restart: no identity, so the app shows onboarding', JSON.stringify(id)?.slice(0, 80));
  const afterRestart = files();
  console.log(`data dir after the restart: ${afterRestart.join(' ') || 'empty'}`);
  console.log(`core.db after the restart: ${db()}`);
  if (existsSync(join(DATA, 'core.db'))) {
    const raw = readFileSync(join(DATA, 'core.db'));
    for (const t of ['sign-out check', 'before sign-out', 'Sign-out QA']) console.log(`  leftover core.db contains "${t}" in plain text: ${raw.includes(Buffer.from(t))}`);
  }
  await again.close();
  // The sign-out may finish on the next start (the "signed-out" marker is the intent).
  const fin = files();
  for (const f of ['core.db', 'core.db-wal', 'core.db-shm', 'sync', 'signed-out']) {
    if (after.includes(f)) result(!fin.includes(f) || (f === 'core.db' && fin.includes(f) && !after.includes('signed-out')), `after the restart: no old ${f}`, fin.join(' ') || 'data dir empty');
  }
} catch (e) {
  result(false, 'the check stopped', e.message);
}
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
