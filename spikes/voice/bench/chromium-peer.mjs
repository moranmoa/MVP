// One Chromium participant for the Docker bench (one container = one peer,
// so the container's cgroup gives CPU and memory per participant).
//
// Env: SIGNAL=http://signal:8080  ROOM  DURATION (s)  NETEM ("delay 50ms loss 2%")
//      SRC=pulses|mic  PROCESSING=1|0  RESULTS=/results  WARMUP (s, default 10)

import { execSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { hostname } from 'node:os';
import { chromium } from 'playwright-core';

const env = process.env;
const id = env.PEER_ID || `c-${hostname().slice(0, 6)}`;
// END_AT (epoch s, set by run.sh) makes every participant stop together.
const duration = env.END_AT ? Math.round(Number(env.END_AT) - Date.now() / 1000) : Number(env.DURATION || 60);
const warmup = Number(env.WARMUP || 10);

if (env.NETEM) execSync(`tc qdisc replace dev eth0 root netem ${env.NETEM}`, { stdio: 'inherit' });

const cgroup = () => {
  const cpu = Object.fromEntries(readFileSync('/sys/fs/cgroup/cpu.stat', 'utf8').trim().split('\n').map((l) => l.split(' ')));
  const mem = Number(readFileSync('/sys/fs/cgroup/memory.current', 'utf8'));
  return { cpuUs: Number(cpu.usage_usec), mem, t: Date.now() };
};

const signal = env.SIGNAL || 'http://signal:8080';
const browser = await chromium.launch({
  args: [
    '--use-fake-ui-for-media-stream',
    '--use-fake-device-for-media-stream',
    '--autoplay-policy=no-user-gesture-required',
    // Containers have no mDNS; expose host candidates as plain IPs.
    '--disable-features=WebRtcHideLocalIpsWithMdns',
  ],
});
const page = await browser.newPage();
page.on('console', (m) => (env.VERBOSE || m.type() === 'error') && console.log(id, m.text()));
page.on('pageerror', (e) => console.error(id, 'pageerror', e.message));
const proc = env.PROCESSING === '0' ? '&aec=0&ns=0&agc=0' : '';
// AudioWorklet and getUserMedia need a secure context, which plain http to
// the signal container is not; localhost is. Serve the client as localhost
// and proxy it from the signal server (the WebSocket still goes there).
await page.route('http://localhost:8080/**', async (route) => {
  const res = await fetch(route.request().url().replace('http://localhost:8080', signal));
  await route.fulfill({ status: res.status, headers: Object.fromEntries(res.headers), body: Buffer.from(await res.arrayBuffer()) });
});
const ws = signal.replace(/^http/, 'ws');
await page.goto(`http://localhost:8080/?signal=${ws}&room=${env.ROOM}&name=${id}&src=${env.SRC || 'pulses'}&autojoin=1&duration=${duration}${proc}`);

await page.waitForTimeout(warmup * 1000);
const a = cgroup();
await page.waitForFunction(() => window.__done === true, null, { timeout: (duration + 30) * 1000 });
const b = cgroup();
const report = await page.evaluate(() => window.__report);
await browser.close();

report.netem = env.NETEM || 'none';
report.cpuPctOfOneCore = Math.round((b.cpuUs - a.cpuUs) / (b.t - a.t)) / 10;
report.memMiB = Math.round(b.mem / 1048576);
mkdirSync(`${env.RESULTS}/${env.ROOM}`, { recursive: true });
writeFileSync(`${env.RESULTS}/${env.ROOM}/${id}.json`, JSON.stringify(report, null, 2));
console.log(id, 'cpu', report.cpuPctOfOneCore, '% mem', report.memMiB, 'MiB', JSON.stringify(report.peers));
