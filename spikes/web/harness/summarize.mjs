// Prints markdown tables from one or more results.json files (one per run).
// Usage: node summarize.mjs dist/results.json [more runs...]

import { readFileSync } from 'node:fs';

const runs = process.argv.slice(2).map((f) => JSON.parse(readFileSync(f, 'utf8')));
const ms = (s) => (s ? `${s.p50} / ${s.p95}` : '–');
const kb = (n) => `${(n / 1024).toFixed(0)} KB`;

const b = runs[0].bundle;
console.log('## Bundle\n');
console.log('| File | Raw | gzip -9 | brotli -q 11 |\n|---|---|---|---|');
console.log(`| web_peer_bg.wasm (after wasm-opt -Oz) | ${kb(b.wasm)} | ${kb(b.wasm_gzip)} | ${kb(b.wasm_brotli)} |`);
console.log(`| web_peer.js (wasm-bindgen glue) | ${kb(b.js)} | ${kb(b.js_gzip)} | |`);

for (const name of ['chromium', 'firefox', 'webkit']) {
  const per = runs.map((r) => r.browsers.find((x) => x.browser === name)).filter(Boolean);
  if (!per.length) continue;
  console.log(`\n## ${name} ${per[0].version}\n`);
  console.log(`wasm init (ms, per run): ${per.map((p) => p.wasm_init_ms?.toFixed(0)).join(', ')}\n`);
  console.log(`| Connect (ms p50 / p95) | ${per.map((_, i) => `run ${i + 1}`).join(' | ')} |`);
  console.log(`|---|${per.map(() => '---').join('|')}|`);
  for (const kind of Object.keys(per[0].connect ?? {})) {
    console.log(`| ${kind} | ${per.map((p) => { const c = p.connect[kind]; return `${c.ok}/${c.of}: ${ms(c.ms)}`; }).join(' | ')} |`);
  }
  for (const label of (per[0].exchange ?? []).map((e) => e.label)) {
    const ex = per.map((p) => p.exchange.find((e) => e.label === label));
    console.log(`\n**${label}**\n`);
    console.log(`| Measure | ${ex.map((_, i) => `run ${i + 1}`).join(' | ')} |`);
    console.log(`|---|${ex.map(() => '---').join('|')}|`);
    const row = (title, f) => console.log(`| ${title} | ${ex.map((e) => (e.error ? `FAILED: ${e.error.slice(0, 80)}` : f(e))).join(' | ')} |`);
    row('connect (ms)', (e) => e.circuit_connect_ms.toFixed(0) + (e.upgrade_to_direct_ms ? ` + direct ${e.upgrade_to_direct_ms.toFixed(0)}` : ''));
    row('connection used', (e) => (e.connections.every((c) => c.relayed) ? 'relayed' : e.connections.every((c) => !c.relayed) ? 'direct' : 'mixed'));
    row('history pulled on connect', (e) => `${e.history_blobs} blobs, ${e.history_ms} ms`);
    row('native → browser latency p50 / p95', (e) => ms(e.native_to_browser.latency_ms));
    row('browser → native latency p50 / p95', (e) => ms(e.browser_to_native.latency_ms));
    row('same state hash', (e) => (e.same_state ? `yes (${e.count.browser})` : 'NO'));
    row('sync round trip p50 / p95', (e) => ms(e.sync_rtt_ms));
    row('ping p50 / p95', (e) => Object.entries(e.ping_ms).map(([k, v]) => `${k} ${ms(v)}`).join(', '));
    if (ex[0]?.bulk) row(`bulk ${kb(ex[0].bulk.bytes)} over circuit`, (e) => (e.bulk.ok ? `ok, ${e.bulk.ms} ms` : `cut after ${e.bulk.received} blobs`));
  }
}

for (const name of ['chromium', 'firefox', 'webkit']) {
  const per = runs.map((r) => r.browsers.find((x) => x.browser === name)).filter((p) => p?.catch_up);
  if (!per.length) continue;
  console.log(`\n**${name}: a new peer joins and catches up**\n`);
  console.log(`| Case | ${per.map((_, i) => `run ${i + 1}`).join(' | ')} |`);
  console.log(`|---|${per.map(() => '---').join('|')}|`);
  for (const label of per[0].catch_up.map((c) => c.label)) {
    const cs = per.map((p) => p.catch_up.find((c) => c.label === label));
    console.log(`| ${label} (${cs[0].blobs} blobs) | ${cs.map((c) => (c.ok ? `ok, ${c.ms} ms` : `FAILED (${c.received ?? 0} received)`)).join(' | ')} |`);
  }
}
