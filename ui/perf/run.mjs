// B14: UI performance harness. Builds the UI, serves dist/ with `vite preview`
// and drives the mock demo in headless Chromium (the same engine as the
// Electron shell), measuring inside the page:
//
//   echo     Enter in the composer → the message on screen as "pending" (B6: < 50 ms)
//   switch   click on a channel → its messages on screen
//   scroll   frame times while scrolling up through a 5,000-message channel
//
// Every measure runs at normal speed and with the CPU throttled 4× (a slower
// laptop). Run inside the Playwright container (see README.md):
//   docker compose run --rm shots npm run perf
// Writes perf/out/results.json and prints the tables for RESULTS.md.
//
// Environment: PERF_SENDS (default 30), PERF_SWITCHES (default 20),
// PERF_SCROLL_STEP (px per frame, default 400), PERF_THROTTLE (default "1,4"),
// PERF_ONLY (default "echo,switch,scroll"; e.g. "scroll" for a quick check),
// PERF_DIST (serve this prebuilt dist directory instead of building).

import { execSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { cpus, loadavg } from 'node:os';
import { chromium } from 'playwright';
import { preview } from 'vite';

const PORT = 4174;
const BASE = `http://localhost:${PORT}/`;
const SENDS = Number(process.env.PERF_SENDS ?? 30);
const SWITCHES = Number(process.env.PERF_SWITCHES ?? 20);
const SCROLL_STEP = Number(process.env.PERF_SCROLL_STEP ?? 400);
const DIST = process.env.PERF_DIST;
const ONLY = (process.env.PERF_ONLY ?? 'echo,switch,scroll').split(',');
const THROTTLE = (process.env.PERF_THROTTLE ?? '1,4').split(',').map(Number);
const ECHO_TARGET_MS = 50;

if (!DIST && !process.argv.includes('--no-build')) execSync('npx vite build', { stdio: 'inherit' });
const server = await preview({ build: { outDir: DIST ?? 'dist' }, preview: { port: PORT, strictPort: true, host: '127.0.0.1' }, logLevel: 'warn' });
const browser = await chromium.launch();

// ---- in-page helpers ---------------------------------------------------------

// Installed before the app loads. `afterPaint` resolves once the frame that
// shows the current DOM has been produced (rAF, then a task after it).
function installHelpers() {
  const afterPaint = () => new Promise((r) => requestAnimationFrame(() => setTimeout(r, 0)));
  window.__perf = {
    longTasks: [],
    /**
     * Resolves when `ready()` holds: { dom: ms to the DOM change, paint: ms to
     * the frame }. Checked on every DOM change and every frame (a click on the
     * already open channel changes nothing); rejects after 15 s.
     */
    until(ready, t0) {
      return new Promise((resolve, reject) => {
        let finished = false;
        const mo = new MutationObserver(() => check());
        const timer = setTimeout(() => finish(() => reject(new Error('timed out'))), 15_000);
        const finish = (f) => {
          if (finished) return;
          finished = true;
          mo.disconnect();
          clearTimeout(timer);
          f();
        };
        const check = () => {
          if (finished || !ready()) return false;
          const dom = performance.now() - t0();
          finish(() => afterPaint().then(() => resolve({ dom, paint: performance.now() - t0() })));
          return true;
        };
        const poll = () => {
          if (!check() && !finished) requestAnimationFrame(poll);
        };
        mo.observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
        poll();
      });
    },
  };
  try {
    new PerformanceObserver((list) => window.__perf.longTasks.push(...list.getEntries().map((e) => e.duration))).observe({
      type: 'longtask',
      buffered: true,
    });
  } catch {
    // Not every engine reports long tasks.
  }
}

// ---- stats ---------------------------------------------------------------------

const round = (x) => Math.round(x * 10) / 10;
function stats(values) {
  const s = [...values].sort((a, b) => a - b);
  const q = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  return { n: s.length, p50: round(q(0.5)), p95: round(q(0.95)), max: round(s[s.length - 1]) };
}

// ---- one session ---------------------------------------------------------------

async function openApp(throttle) {
  const ctx = await browser.newContext({ viewport: { width: 1366, height: 800 }, locale: 'en-US' });
  const page = await ctx.newPage();
  await page.addInitScript(installHelpers);
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  if (throttle > 1) {
    const cdp = await ctx.newCDPSession(page);
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: throttle });
  }
  // Demo account, no simulated activity, no update banner.
  await page.goto(`${BASE}?fresh=1&demo=1&still=1&lang=en&theme=light&update=999999`);
  await page.locator('.composer textarea').waitFor();
  return { ctx, page, errors };
}

const openServer = (page, name) => page.getByRole('navigation').getByRole('button', { name }).first().click();
const channelButton = (page, name) => page.locator('.channel-row button').filter({ has: page.locator('.channel-name', { hasText: new RegExp(`^${name}$`) }) });

/** Clicks a channel and resolves when its messages (not placeholders) are on screen. */
async function switchTo(page, name) {
  await page.evaluate((name) => {
    let t0 = 0;
    document.addEventListener('click', () => (t0 = performance.now()), { capture: true, once: true });
    window.__switch = window.__perf.until(() => {
      const feed = document.querySelector('[role=feed]');
      return (
        t0 > 0 &&
        feed?.getAttribute('aria-label')?.includes(name) &&
        feed.getAttribute('aria-busy') === 'false' &&
        !!feed.querySelector('.row .msg-body')
      );
    }, () => t0);
  }, name);
  await channelButton(page, name).click();
  return page.evaluate(() => window.__switch);
}

async function measureEcho(page) {
  await openServer(page, 'Minecraft IL');
  await switchTo(page, 'general');
  const composer = page.locator('.composer textarea');
  const out = [];
  for (let i = 0; i < SENDS; i++) {
    const body = `perf echo ${i} ${Date.now()}`;
    await composer.fill(body);
    await page.evaluate((body) => {
      let t0 = 0;
      document.addEventListener('keydown', (e) => (t0 = e.timeStamp || performance.now()), { capture: true, once: true });
      window.__echo = window.__perf.until(() => {
        if (!t0) return false;
        for (const row of document.querySelectorAll('[role=feed] .row')) {
          if (row.textContent.includes(body) && row.querySelector('.send-state.pending')) return true;
        }
        return false;
      }, () => t0);
    }, body);
    await composer.press('Enter');
    out.push(await page.evaluate(() => window.__echo));
    await page.waitForTimeout(150);
  }
  return { dom: stats(out.map((o) => o.dom)), paint: stats(out.map((o) => o.paint)) };
}

async function measureSwitch(page) {
  const out = { small: [], large: [] };
  // Pairs in one server: a small channel and a large one, alternating.
  for (const [server, small, large] of [
    ['Minecraft IL', 'builds', 'general'],
    ['גיימרים ישראל', 'clips', 'כללי'],
  ]) {
    await openServer(page, server);
    await page.waitForTimeout(300);
    for (let i = 0; i < SWITCHES / 2; i++) {
      for (const [kind, name] of [['small', small], ['large', large]]) {
        out[kind].push((await switchTo(page, name)).paint);
        await page.waitForTimeout(100);
      }
    }
  }
  return { to_small: stats(out.small), to_large: stats(out.large) };
}

async function measureScroll(page) {
  await openServer(page, 'Minecraft IL');
  await switchTo(page, 'general');
  await page.waitForTimeout(500);
  return page.evaluate(async (step) => {
    const feed = document.querySelector('[role=feed]');
    window.__perf.longTasks.length = 0;
    const frames = [];
    let placeholderFrames = 0;
    let last = performance.now();
    const view = feed.getBoundingClientRect();
    await new Promise((done) => {
      const tick = (now) => {
        frames.push(now - last);
        last = now;
        const blank = [...feed.querySelectorAll('.msg.skeleton')].some((el) => {
          const r = el.getBoundingClientRect();
          return r.bottom > view.top && r.top < view.bottom;
        });
        if (blank) placeholderFrames++;
        if (feed.scrollTop <= 0 || frames.length > 2000) return done();
        feed.scrollTop -= step;
        requestAnimationFrame(tick);
      };
      requestAnimationFrame((now) => {
        last = now;
        requestAnimationFrame(tick);
      });
    });
    const firstIndex = Number(feed.querySelector('.row')?.getAttribute('data-index'));
    return { frames, placeholderFrames, longTasks: [...window.__perf.longTasks], reachedIndex: firstIndex };
  }, SCROLL_STEP);
}

function frameStats(r) {
  const f = r.frames.slice(1);
  const over = (ms) => round((100 * f.filter((d) => d > ms).length) / f.length);
  return {
    frames: f.length,
    ...stats(f),
    p99: round([...f].sort((a, b) => a - b)[Math.floor(0.99 * f.length)]),
    // 60 Hz frames are 16.7 ms; timer jitter puts many at 16.7–16.8, so a
    // frame counts as missed above 20 ms, and as two missed above 34 ms.
    missed_frame_pct: over(20),
    missed_two_pct: over(34),
    placeholder_frames_pct: round((100 * r.placeholderFrames) / f.length),
    long_tasks: r.longTasks.length,
    long_task_ms: round(r.longTasks.reduce((a, b) => a + b, 0)),
    reached_top: r.reachedIndex === 0,
  };
}

// ---- main ----------------------------------------------------------------------

const results = {
  date: new Date().toISOString().slice(0, 10),
  browser: `chromium ${browser.version()}`,
  cpus: cpus().length,
  sends: SENDS,
  switches: SWITCHES,
  scroll_step_px: SCROLL_STEP,
  runs: [],
};
for (const throttle of THROTTLE) {
  const { ctx, page, errors } = await openApp(throttle);
  // Load average of the (shared) Docker VM: the 4x numbers swing with it.
  const run = { cpu_throttle: throttle, load_before: round(loadavg()[0]) };
  if (ONLY.includes('echo')) {
    run.echo = await measureEcho(page);
    console.log(`[${throttle}x] echo`, JSON.stringify(run.echo));
  }
  if (ONLY.includes('switch')) {
    run.channel_switch = await measureSwitch(page);
    console.log(`[${throttle}x] switch`, JSON.stringify(run.channel_switch));
  }
  if (ONLY.includes('scroll')) {
    run.scroll_5000 = frameStats(await measureScroll(page));
    console.log(`[${throttle}x] scroll`, JSON.stringify(run.scroll_5000));
  }
  run.load_after = round(loadavg()[0]);
  run.page_errors = errors;
  results.runs.push(run);
  await ctx.close();
}
await browser.close();
server.httpServer.close();

mkdirSync('perf/out', { recursive: true });
writeFileSync('perf/out/results.json', JSON.stringify(results, null, 2));

// Tables for RESULTS.md.
const cols = results.runs.map((r) => `CPU ${r.cpu_throttle}×`);
const row = (label, f) => console.log(`| ${label} | ${results.runs.map(f).join(' | ')} |`);
const pq = (s) => `${s.p50} / ${s.p95} (max ${s.max})`;
console.log(`\n${results.browser}, ${results.cpus} CPUs, ${results.date}. ms, p50 / p95.`);
console.log(`Host load (1-min average, before / after): ${results.runs.map((r) => `${r.load_before} / ${r.load_after}`).join(', ')}\n`);
// Other containers' builds take CPU from the browser, and the 4x runs show
// it most (see RESULTS.md, "Host load"). Such runs aren't comparable.
const busy = results.runs.some((r) => Math.max(r.load_before, r.load_after) > results.cpus / 2);
if (busy) console.log(`WARNING: the host was busy (load above ${results.cpus / 2}); rerun before comparing numbers.\n`);
console.log(`| Measure | ${cols.join(' | ')} |\n|---|${cols.map(() => '---').join('|')}|`);
if (ONLY.includes('echo')) {
  row(`pending echo, to DOM (n=${SENDS})`, (r) => pq(r.echo.dom));
  row(`pending echo, to screen (target < ${ECHO_TARGET_MS})`, (r) => pq(r.echo.paint));
}
if (ONLY.includes('switch')) {
  row('channel switch, to a small channel (300–500 msgs)', (r) => pq(r.channel_switch.to_small));
  row('channel switch, to a large channel (5,000–100,000 msgs)', (r) => pq(r.channel_switch.to_large));
}
if (ONLY.includes('scroll')) {
  row('scroll 5,000 msgs: frame time', (r) => `${r.scroll_5000.p50} / ${r.scroll_5000.p95} (p99 ${r.scroll_5000.p99})`);
  row('scroll: frames over 20 / 34 ms (missed 1 / 2+ frames)', (r) => `${r.scroll_5000.missed_frame_pct}% / ${r.scroll_5000.missed_two_pct}%`);
  row('scroll: frames showing placeholders', (r) => `${r.scroll_5000.placeholder_frames_pct}%`);
  row('scroll: long tasks (count, total ms)', (r) => `${r.scroll_5000.long_tasks}, ${r.scroll_5000.long_task_ms}`);
  row('scroll: frames, reached the top', (r) => `${r.scroll_5000.frames}, ${r.scroll_5000.reached_top ? 'yes' : 'no'}`);
}
const failed = results.runs.some((r) => r.page_errors.length);
if (failed) console.log('page errors:', results.runs.map((r) => r.page_errors));
process.exit(failed ? 1 : 0);
