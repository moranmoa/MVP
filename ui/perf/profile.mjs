// Where does scroll time go? Builds the UI unminified (readable function
// names), scrolls the 5,000-message channel like run.mjs, under the Chrome
// CPU profiler, and prints the functions with the most self time.
//   docker compose run --rm shots node perf/profile.mjs
// Environment: PERF_THROTTLE (single factor, default 4), PERF_SCROLL_STEP (400).

import { execSync } from 'node:child_process';
import { chromium } from 'playwright';
import { preview } from 'vite';

const PORT = 4175;
const THROTTLE = Number(process.env.PERF_THROTTLE ?? 4);
const STEP = Number(process.env.PERF_SCROLL_STEP ?? 400);
const OUT = 'perf/out/dist-profile';

execSync(`npx vite build --minify false --outDir ${OUT} --emptyOutDir`, { stdio: 'inherit' });
const server = await preview({ build: { outDir: OUT }, preview: { port: PORT, strictPort: true, host: '127.0.0.1' }, logLevel: 'warn' });
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1366, height: 800 }, locale: 'en-US' });
const page = await ctx.newPage();
const cdp = await ctx.newCDPSession(page);
await page.goto(`http://localhost:${PORT}/?fresh=1&demo=1&still=1&lang=en&theme=light&update=999999`);
await page.locator('.composer textarea').waitFor();
await page.getByRole('navigation').getByRole('button', { name: 'Minecraft IL' }).first().click();
await page.locator('[role=feed][aria-busy=false] .row .msg-body').first().waitFor();
await page.waitForTimeout(500);

await cdp.send('Emulation.setCPUThrottlingRate', { rate: THROTTLE });
await cdp.send('Profiler.enable');
await cdp.send('Profiler.setSamplingInterval', { interval: 200 });
await cdp.send('Profiler.start');
await page.evaluate(
  (step) =>
    new Promise((done) => {
      const feed = document.querySelector('[role=feed]');
      const tick = () => {
        if (feed.scrollTop <= 0) return done();
        feed.scrollTop -= step;
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    }),
  STEP,
);
const { profile } = await cdp.send('Profiler.stop');
await browser.close();
server.httpServer.close();

// Self time per function (sample count × interval).
const dt = new Map();
profile.samples.forEach((id, i) => dt.set(id, (dt.get(id) ?? 0) + (profile.timeDeltas[i] ?? 0)));
const byFn = new Map();
let total = 0;
for (const node of profile.nodes) {
  const us = dt.get(node.id) ?? 0;
  total += us;
  const { functionName, url, lineNumber } = node.callFrame;
  const key = `${functionName || '(anonymous)'} ${url ? url.split('/').pop() : ''}${url ? `:${lineNumber + 1}` : ''}`;
  byFn.set(key, (byFn.get(key) ?? 0) + us);
}
console.log(`\nCPU ${THROTTLE}×, total sampled ${(total / 1000).toFixed(0)} ms. Top self time:\n`);
for (const [key, us] of [...byFn].sort((a, b) => b[1] - a[1]).slice(0, 30)) {
  console.log(`${(us / 1000).toFixed(0).padStart(7)} ms  ${((100 * us) / total).toFixed(1).padStart(5)}%  ${key}`);
}
