// The app with the REAL core (mvs-core from core/api) behind the bridge:
// create an identity, read the words back, pass the word check, create a
// server, and make sure the recovery words never reach the main process's
// output (stdout or stderr, which is where its log goes).
//
// Needs MVS_CORE_PATH (absolute) and a working OS keychain; in Docker that is
// a session D-Bus with an unlocked gnome-keyring (compose service real-core).
import test, { after, before } from 'node:test';
import assert from 'node:assert/strict';
import { _electron as electron } from 'playwright-core';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CORE = process.env.MVS_CORE_PATH;
let app;
// A throwaway profile, never the real app's.
const PROFILE = mkdtempSync(join(tmpdir(), 'mvs-e2e-core-'));
let page;
let mainOutput = '';

const invoke = (cmd, args = []) => page.evaluate(([c, a]) => window.mvs.invoke(c, a), [cmd, args]);

before(async () => {
  assert.ok(CORE, 'set MVS_CORE_PATH to the mvs-core binary');
  app = await electron.launch({ args: ['.'], env: { ...process.env, MVS_TEST_USER_DATA: PROFILE } });
  const proc = app.process();
  proc.stdout.on('data', (d) => (mainOutput += d));
  proc.stderr.on('data', (d) => (mainOutput += d));
  page = await app.firstWindow();
  await page.waitForFunction(() => typeof window.mvs?.invoke === 'function');
});

after(async () => {
  await app?.close();
  rmSync(PROFILE, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
});

let words;

test('createIdentity returns an identity and the recovery words', async () => {
  const res = await invoke('createIdentity', [{ locale: 'he', country: 'IL', birthDate: '1998-04-12' }]);
  assert.equal(res.error, undefined, JSON.stringify(res.error));
  assert.ok(res.ok.identity, 'identity');
  words = res.ok.words;
  assert.ok(Array.isArray(words) && words.length >= 12, `words: ${words?.length}`);
});

test('getRecoveryWords gives the same words before the check', async () => {
  const res = await invoke('getRecoveryWords');
  assert.deepEqual(res.ok, words);
});

test('the word check passes with the right words', async () => {
  // getRecoveryChallenge and the { ok } result are B2-f (dev-identity); until
  // then the core takes any positions and answers a plain boolean.
  const challenge = await invoke('getRecoveryChallenge');
  const positions = challenge.ok?.positions ?? [1, 5, 9];
  const answers = Object.fromEntries(positions.map((p) => [p, words[p - 1]]));
  const res = await invoke('checkRecoveryWords', [answers]);
  assert.equal(res.error, undefined, JSON.stringify(res.error));
  assert.ok(res.ok === true || res.ok?.ok === true, JSON.stringify(res.ok));
  // Once checked, the words cannot be read again (until the app lock, #31).
  assert.deepEqual(await invoke('getRecoveryWords'), { error: { code: 'not-allowed' } });
});

test('createServer works after the check', async () => {
  const res = await invoke('createServer', [{ name: 'בדיקה e2e', ageRestriction: 'none' }]);
  assert.equal(res.error, undefined, JSON.stringify(res.error));
  assert.equal(res.ok.name, 'בדיקה e2e');
  const list = await invoke('listServers');
  assert.ok(list.ok.some((s) => s.id === res.ok.id));
});

test('the recovery words never appear in the main process output', async () => {
  // Give buffered output a moment to arrive.
  await new Promise((r) => setTimeout(r, 500));
  assert.ok(mainOutput.length > 0, 'main produced some output (core stderr is logged)');
  for (const w of words) {
    assert.ok(!new RegExp(`\\b${w}\\b`).test(mainOutput), `word "${w}" leaked into main's output`);
  }
});
