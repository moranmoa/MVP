// Reads the fuses back from the packaged binary (after `npm run dist:linux`)
// and checks each one against build/fuses.js.
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { getCurrentFuseWire, FuseV1Options } from '@electron/fuses';

const require = createRequire(import.meta.url);
const { FUSES } = require('../build/fuses.js');
const BINARY = process.env.MVS_PACKAGED_BINARY || 'dist/linux-unpacked/mvs';

test('the packaged binary has the fuses we set', async () => {
  assert.ok(existsSync(BINARY), `${BINARY} missing: run npm run dist:linux first`);
  const wire = await getCurrentFuseWire(BINARY);
  for (const [key, want] of Object.entries(FUSES)) {
    if (key === 'version') continue;
    // The wire stores the character '1' (on) or '0' (off) per fuse, as a char code.
    const got = String.fromCharCode(wire[key]);
    assert.equal(got, want ? '1' : '0', `${FuseV1Options[key]}: got ${got}`);
  }
});

test('the app code is only in app.asar and the core is next to it', () => {
  const res = BINARY.replace(/[^/]+$/, 'resources/');
  assert.ok(existsSync(`${res}app.asar`), 'app.asar');
  assert.ok(!existsSync(`${res}app`), 'no unpacked app folder');
  assert.ok(existsSync(`${res}ui/index.html`), 'ui');
  assert.ok(existsSync(`${res}mvs-core`), 'mvs-core');
});

test('the binary will not act as Node, and NODE_OPTIONS is ignored', async () => {
  const { spawnSync } = await import('node:child_process');
  const marker = 'RUN_AS_NODE_MARKER_7f3a';
  const asNode = spawnSync(BINARY, ['-e', `console.log('${marker}')`], {
    env: { ...process.env, ELECTRON_RUN_AS_NODE: '1', DISPLAY: '' },
    encoding: 'utf8',
    timeout: 15_000,
  });
  assert.ok(!`${asNode.stdout}${asNode.stderr}`.includes(marker), 'ELECTRON_RUN_AS_NODE must be off');
  const opts = spawnSync(BINARY, [], {
    env: { ...process.env, NODE_OPTIONS: '--require=/nonexistent-mvs-preload.js', DISPLAY: '' },
    encoding: 'utf8',
    timeout: 15_000,
  });
  assert.ok(!`${opts.stdout}${opts.stderr}`.includes('nonexistent-mvs-preload'), 'NODE_OPTIONS must be ignored');
});
