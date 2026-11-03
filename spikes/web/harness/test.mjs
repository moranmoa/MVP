// Spike 5 harness: headless Chromium and Firefox run the wasm peer against
// the relay, an unmodified core/sync node (`sync-peer`) and the WebRTC
// native peer (`web-native`). Writes results.json to OUT_DIR (summarize.mjs
// turns it into tables).
//
// Environment: RELAY_ID (relay peer ID), SYNC (host:port of sync-peer's
// control port), NATIVE (same for web-native), BROWSERS (default
// "chromium,firefox"; "webkit" works too), ATTEMPTS (connects per kind,
// default 10), MESSAGES (blobs each way, default 50), BULK_BLOBS and
// BLOB_BYTES (bulk tests, default 40 × 8192), PAGE_URL, OUT_DIR, WWW.

import { chromium, firefox, webkit } from 'playwright';
import { createServer } from 'node:http';
import { readFile, writeFile } from 'node:fs/promises';
import { connect } from 'node:net';
import { lookup } from 'node:dns/promises';
import { extname, join } from 'node:path';

const RELAY_ID = process.env.RELAY_ID;
const RELAY_WS = `/dns4/relay/tcp/4002/ws/p2p/${RELAY_ID}`;
const PAGE_URL = process.env.PAGE_URL ?? 'http://browsers:8080/';
const BULK_BLOBS = Number(process.env.BULK_BLOBS ?? 40);
const BLOB_BYTES = Number(process.env.BLOB_BYTES ?? 8192);
const ATTEMPTS = Number(process.env.ATTEMPTS ?? 10);
const MESSAGES = Number(process.env.MESSAGES ?? 50);
const OUT_DIR = process.env.OUT_DIR ?? '/out';
const WWW = process.env.WWW ?? '/www';
const BROWSERS = (process.env.BROWSERS ?? 'chromium,firefox').split(',');

// ---- control ports of the native peers (one JSON command per line) ----

class Control {
  constructor(hostPort) {
    const [host, port] = hostPort.split(':');
    this.host = host;
    this.port = Number(port);
  }
  async send(cmd) {
    if (!this.socket) {
      this.socket = connect(this.port, this.host);
      this.socket.setEncoding('utf8');
      this.buffer = '';
      this.waiting = [];
      this.socket.on('data', (chunk) => {
        this.buffer += chunk;
        let i;
        while ((i = this.buffer.indexOf('\n')) >= 0) {
          const line = this.buffer.slice(0, i);
          this.buffer = this.buffer.slice(i + 1);
          this.waiting.shift()(JSON.parse(line));
        }
      });
      await new Promise((ok, fail) => { this.socket.once('connect', ok); this.socket.once('error', fail); });
    }
    const reply = await new Promise((ok) => { this.waiting.push(ok); this.socket.write(JSON.stringify(cmd) + '\n'); });
    if (reply.error) throw new Error(`${this.host}: ${reply.error}`);
    return reply;
  }
  close() { this.socket?.end(); }
}

async function retry(fn, tries = 60) {
  for (let i = 0; ; i++) {
    try { return await fn(); } catch (err) { if (i >= tries) throw err; await sleep(500); }
  }
}

const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));

async function waitFor(what, fn, timeoutMs = 30000) {
  const started = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return Date.now() - started;
    if (Date.now() - started > timeoutMs) throw new Error(`timed out waiting for ${what}`);
    await sleep(25);
  }
}

// ---- stats ----

function summary(values) {
  if (!values.length) return null;
  const s = [...values].sort((a, b) => a - b);
  const q = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  const r = (x) => Math.round(x * 10) / 10;
  return { n: s.length, p50: r(q(0.5)), p95: r(q(0.95)), min: r(s[0]), max: r(s[s.length - 1]) };
}

// ---- static server for the page and the wasm bundle ----

const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm' };
const server = createServer(async (req, res) => {
  let path = new URL(req.url, 'http://x').pathname;
  if (path === '/') path = '/index.html';
  try {
    const body = await readFile(join(WWW, path));
    res.writeHead(200, { 'content-type': TYPES[extname(path)] ?? 'application/octet-stream' });
    res.end(body);
  } catch {
    res.writeHead(404).end();
  }
});
await new Promise((ok) => server.listen(8080, ok));

// ---- one browser ----

async function runBrowser(name, sync, native, syncId, nativeId) {
  const type = { chromium, firefox, webkit }[name];
  const browser = await type.launch();
  const version = browser.version();
  const page = await browser.newPage();
  const consoleLines = [];
  page.on('console', (m) => consoleLines.push(m.text()));
  page.on('pageerror', (e) => consoleLines.push(`pageerror: ${e}`));

  const loadStarted = Date.now();
  // Not localhost: libp2p-webrtc-websys refuses to dial from a Firefox page
  // served from localhost (Firefox blocks WebRTC to loopback there).
  await page.goto(PAGE_URL);
  await page.waitForFunction(() => window.ready === true, null, { timeout: 30000 }).catch((err) => {
    throw new Error(`${err.message}\nconsole:\n${consoleLines.join('\n')}`);
  });
  const result = {
    browser: name,
    version,
    page_load_to_wasm_ready_ms: Date.now() - loadStarted,
    wasm_init_ms: await page.evaluate(() => window.wasmInitMs),
  };
  console.log(`[${name} ${version}] wasm ready`, result.wasm_init_ms.toFixed(0), 'ms');

  // Each attempt uses a fresh peer (new key, no existing connections), and
  // stops it afterwards so its relay circuits are released.
  const attempt = (addrs, eager) => page.evaluate(async ({ addrs, eager }) => {
    const peer = new window.WebPeer('scratch', eager);
    const out = [];
    for (const addr of addrs) {
      try { out.push(JSON.parse(await peer.dial(addr))); } catch (e) { out.push({ error: String(e) }); }
    }
    peer.stop();
    return out;
  }, { addrs, eager });

  // 1. Connect times: browser → relay (WebSocket), → native through the
  //    relay circuit, → native directly over WebRTC.
  const nativeWebrtc = (await native.send({ cmd: 'state' })).listen_addrs
    .filter((a) => a.includes('/webrtc-direct/') && !a.startsWith('/ip4/127.'))
    .map((a) => `${a}/p2p/${nativeId}`);
  const kinds = {
    // websocket-websys as released (100 ms flush polling), then with EagerFlush.
    relay_ws_stock_flush: [RELAY_WS],
    relay_ws: [RELAY_WS],
    // Same relay by IP: separates the browser's DNS lookup from the connect.
    relay_ws_by_ip: [`/ip4/${(await lookup('relay', { family: 4 })).address}/tcp/4002/ws/p2p/${RELAY_ID}`],
    circuit_to_sync: [`${RELAY_WS}/p2p-circuit/p2p/${syncId}`],
    circuit_to_native: [`${RELAY_WS}/p2p-circuit/p2p/${nativeId}`],
    webrtc_direct_to_native: [nativeWebrtc[0]],
  };
  result.connect = {};
  for (const [kind, addrs] of Object.entries(kinds)) {
    const eager = kind !== 'relay_ws_stock_flush';
    const times = [];
    const errors = [];
    for (let i = 0; i < ATTEMPTS; i++) {
      const [r] = await attempt(addrs, eager);
      if (r.error) errors.push(r.error); else times.push(r.ms);
    }
    result.connect[kind] = { ok: times.length, of: ATTEMPTS, ms: summary(times), errors: [...new Set(errors)] };
    console.log(`[${name}] ${kind}: ${times.length}/${ATTEMPTS}`, JSON.stringify(summary(times)), errors[0] ?? '');
  }

  // 2. Blob exchange with an unmodified core/sync node, over the relay circuit,
  //    then with the WebRTC native peer over a direct connection.
  await page.evaluate(() => { window.peer = new window.WebPeer('general', true); });
  const state = async () => JSON.parse(await page.evaluate(() => window.peer.state()));
  const exchange = async (label, ctl, remoteId, connectDirect) => {
    const out = { label };
    const t0 = Date.now();
    const circuit = JSON.parse(await page.evaluate((a) => window.peer.dial(a), `${RELAY_WS}/p2p-circuit/p2p/${remoteId}`));
    out.circuit_connect_ms = circuit.ms;
    if (connectDirect) {
      // Learn the native peer's WebRTC address through identify on the
      // relayed connection, dial it, then drop the relayed connection.
      await waitFor('identify', async () => ((await state()).identified[remoteId] ?? []).some((a) => a.includes('webrtc-direct')));
      const addr = (await state()).identified[remoteId].find((a) => a.includes('/webrtc-direct/') && !a.startsWith('/ip4/127.'));
      const direct = JSON.parse(await page.evaluate((a) => window.peer.dial(a), `${addr}/p2p/${remoteId}`));
      out.upgrade_to_direct_ms = direct.ms;
      out.relayed_to_direct_total_ms = Date.now() - t0;
      await page.evaluate((p) => window.peer.closeRelayed(p), remoteId);
      await sleep(300);
    }
    out.connections = (await state()).connections.filter((c) => c.peer === remoteId);

    // Catch up on the history the remote already has (pull on connect).
    const remoteCount = (await ctl.send({ cmd: 'state' })).count;
    out.history_blobs = remoteCount;
    out.history_ms = await waitFor('history', async () => (await state()).count >= remoteCount);

    // Native → browser.
    await page.evaluate(() => window.peer.state()); // flush
    const before = (await state()).latencies_ms.length;
    await ctl.send({ cmd: 'reset' });
    await ctl.send({ cmd: 'write', n: MESSAGES, interval_ms: 20 });
    const target = remoteCount + MESSAGES;
    await waitFor('native→browser', async () => (await state()).count >= target);
    const s1 = await state();
    out.native_to_browser = { latency_ms: summary(s1.latencies_ms.slice(before)), via_push: s1.via_push, via_pull: s1.via_pull };

    // Browser → native.
    await page.evaluate(async ({ n, label }) => {
      for (let i = 1; i <= n; i++) {
        const micros = Math.round((performance.timeOrigin + performance.now()) * 1000);
        await window.peer.append(`browser-${label}|${i}|${micros}|הודעה מהדפדפן ${i}`);
        await new Promise((ok) => setTimeout(ok, 20));
      }
    }, { n: MESSAGES, label });
    const total = (await state()).count;
    await waitFor('browser→native', async () => (await ctl.send({ cmd: 'state' })).count >= total);
    const lat = await ctl.send({ cmd: 'latencies' });
    out.browser_to_native = { latency_ms: summary(lat.latencies_ms), via_push: lat.via_push, via_pull: lat.via_pull };

    // Same blob set on both sides.
    const [b, n] = [await state(), await ctl.send({ cmd: 'state' })];
    out.same_state = b.hash === n.hash;
    out.count = { browser: b.count, native: n.count };

    // Round trips: a sync request answered with an empty page, and ping.
    // 20 samples: core/sync rate-limits pulls (burst 30, then 10/s per peer).
    out.sync_rtt_ms = summary(JSON.parse(await page.evaluate((p) => window.peer.rtt(p, 20), remoteId)).ms);
    out.ping_ms = Object.fromEntries(Object.entries((await state()).ping_ms).map(([k, v]) => [k, summary(v)]));
    if (!connectDirect) out.bulk = await bulk(ctl, remoteId);
    console.log(`[${name}] ${label}:`, JSON.stringify(out));
    return out;
  };

  // Bulk push over the relayed connection: does the relay's per-circuit
  // byte limit (libp2p default 128 KiB) cut it off?
  const bulk = async (ctl, remoteId) => {
    const before = (await ctl.send({ cmd: 'state' })).count;
    const started = Date.now();
    await page.evaluate(async ({ n, size }) => {
      const filler = 'x'.repeat(size);
      for (let i = 1; i <= n; i++) await window.peer.append(`browser-bulk|${i}|-|${filler}`);
    }, { n: BULK_BLOBS, size: BLOB_BYTES });
    const out = { blobs: BULK_BLOBS, bytes: BULK_BLOBS * BLOB_BYTES };
    try {
      out.ms = await waitFor('bulk', async () => (await ctl.send({ cmd: 'state' })).count >= before + BULK_BLOBS, 20000);
      out.ok = true;
    } catch {
      out.ok = false;
      out.received = (await ctl.send({ cmd: 'state' })).count - before;
    }
    out.connection_open_after = (await state()).connections.some((c) => c.peer === remoteId);
    out.closed_events = (await state()).events.filter((e) => e.includes('closed')).slice(-3);
    console.log(`[${name}] bulk over circuit:`, JSON.stringify(out), `${Date.now() - started} ms`);
    return out;
  };

  result.exchange = [];
  for (const [label, ctl, id, direct] of [
    ['core/sync node, relay circuit', sync, syncId, false],
    ['web-native, WebRTC direct', native, nativeId, true],
  ]) {
    try {
      result.exchange.push(await exchange(label, ctl, id, direct));
    } catch (err) {
      result.exchange.push({ label, error: String(err), state: await state().catch(() => null) });
      console.log(`[${name}] ${label} FAILED: ${err}`);
    }
  }
  // 3. A new browser peer joins and pulls the whole history in one go (large
  //    sync responses): from the core/sync node through the relay, and from
  //    web-native over WebRTC only (no relay connection at all).
  result.catch_up = [];
  const webrtcAddr = (await native.send({ cmd: 'state' })).listen_addrs
    .find((a) => a.includes('/webrtc-direct/') && !a.startsWith('/ip4/127.'));
  for (const [label, ctl, addr] of [
    ['core/sync node, relay circuit', sync, `${RELAY_WS}/p2p-circuit/p2p/${syncId}`],
    ['web-native, WebRTC direct only', native, `${webrtcAddr}/p2p/${nativeId}`],
  ]) {
    const remote = await ctl.send({ cmd: 'state' });
    const out = { label, blobs: remote.count };
    try {
      const started = Date.now();
      await page.evaluate((a) => { window.joiner = new window.WebPeer('general', true); return window.joiner.dial(a); }, addr);
      const count = async () => JSON.parse(await page.evaluate(() => window.joiner.state())).count;
      out.ms = await waitFor('catch-up', async () => (await count()) >= remote.count);
      out.total_ms = Date.now() - started;
      out.ok = true;
    } catch (err) {
      out.ok = false;
      out.error = String(err);
      out.received = JSON.parse(await page.evaluate(() => window.joiner?.state() ?? '{}')).count;
    }
    await page.evaluate(() => window.joiner?.stop());
    console.log(`[${name}] catch-up ${label}:`, JSON.stringify(out));
    result.catch_up.push(out);
  }

  // The other direction: a browser that wrote 320 KB offline connects over
  //    WebRTC only, and the native peer pulls it all (a large response sent
  //    by the browser).
  {
    const before = (await native.send({ cmd: 'state' })).count;
    // Known to fail with libp2p-webrtc 0.8.0-alpha / webrtc-websys 0.4.0-alpha
    // for blobs of ~8 KB (RESULTS.md); reported, but not counted as a failure.
    const out = { label: 'web-native pulls from browser, WebRTC direct only', blobs: BULK_BLOBS, bytes: BULK_BLOBS * BLOB_BYTES, known_issue: true };
    try {
      await page.evaluate(async ({ n, size }) => {
        window.offline = new window.WebPeer('general', true);
        const filler = 'y'.repeat(size);
        for (let i = 1; i <= n; i++) await window.offline.append(`browser-offline|${i}|-|${filler}`);
      }, { n: BULK_BLOBS, size: BLOB_BYTES });
      const started = Date.now();
      await page.evaluate((a) => window.offline.dial(a), `${webrtcAddr}/p2p/${nativeId}`);
      out.ms = await waitFor('native pull', async () => (await native.send({ cmd: 'state' })).count >= before + BULK_BLOBS);
      out.total_ms = Date.now() - started;
      out.ok = true;
    } catch (err) {
      out.ok = false;
      out.error = String(err);
      out.received = (await native.send({ cmd: 'state' })).count - before;
      out.browser_events = JSON.parse(await page.evaluate(() => window.offline.state())).events;
    }
    await page.evaluate(() => window.offline?.stop());
    console.log(`[${name}] catch-up ${out.label}:`, JSON.stringify(out));
    result.catch_up.push(out);
  }

  result.console_tail = consoleLines.slice(-40);
  await browser.close();
  return result;
}

// ---- main ----

const sync = new Control(process.env.SYNC);
const native = new Control(process.env.NATIVE);
const syncId = (await retry(() => sync.send({ cmd: 'start' }))).peer_id;
const nativeId = (await retry(() => native.send({ cmd: 'start' }))).peer_id;
console.log('relay', RELAY_ID, 'sync-peer', syncId, 'web-native', nativeId);
// Let both register their circuit reservations at the relay.
await sleep(2000);
await native.send({ cmd: 'write', n: 20, interval_ms: 0 });
await sync.send({ cmd: 'write', n: 20, interval_ms: 0 });
await sleep(500);

const bundle = JSON.parse(await readFile(join(WWW, 'pkg', 'sizes.json'), 'utf8').catch(() => 'null'));
const results = { date: new Date().toISOString(), messages: MESSAGES, attempts: ATTEMPTS, bundle, browsers: [] };
for (const name of BROWSERS) {
  try {
    results.browsers.push(await runBrowser(name, sync, native, syncId, nativeId));
  } catch (err) {
    console.log(`[${name}] FAILED`, err);
    results.browsers.push({ browser: name, error: String(err) });
  }
}
await writeFile(join(OUT_DIR, 'results.json'), JSON.stringify(results, null, 2));
console.log('wrote', join(OUT_DIR, 'results.json'));
sync.close();
native.close();
server.close();
const failed = results.browsers.some((b) =>
  b.error || b.exchange?.some((e) => e.error || !e.same_state) || b.catch_up?.some((c) => !c.ok && !c.known_issue));
process.exit(failed ? 1 : 0);
